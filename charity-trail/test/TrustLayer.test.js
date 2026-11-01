const { expect } = require("chai");
const { ethers } = require("hardhat");
const { loadFixture, time } = require("@nomicfoundation/hardhat-network-helpers");
const { usd, hash, sha, org, deployWorld, createEvent } = require("./helpers");

const WINDOW = 3600;
// Kayes region, Mali (demo coordinates), degrees * 1e5.
const LAT = 1445000;
const LON = -1144000;

async function world() {
  const s = await ethers.getSigners();
  const [, , globalNgo, globalNgo2, localNgo, villageRep, auditor, arbiter, driller, donor, visitor, nobody] = s;
  const w = await deployWorld({
    "Blue Horizon Foundation": [globalNgo, "blue-horizon"],
    "Blue Horizon (2nd wallet)": [globalNgo2, "blue-horizon"],
    "Kayes Water Collective": [localNgo, "kayes-water"],
    "Village council": [villageRep, "village-council"],
    "Sahel Audit": [auditor, "sahel-audit"],
    Ombudsman: [arbiter, "ombudsman"],
  });
  const U = await w.usdc.getAddress();

  const parent = await createEvent(w.registry, globalNgo, {
    name: "Clean water Sahel",
    arbiter: arbiter.address,
    attesters: [localNgo.address, auditor.address, villageRep.address],
    window: WINDOW,
    tokens: [U],
  });
  const child = await createEvent(w.registry, localNgo, {
    name: "Wells in Kayes",
    arbiter: arbiter.address,
    attesters: [villageRep.address, auditor.address],
    window: WINDOW,
    tokens: [U],
  });

  await w.usdc.mint(donor.address, usd(1000));
  await w.usdc.connect(donor).approve(await parent.getAddress(), ethers.MaxUint256);
  await parent.connect(donor).donate(U, usd(1000));

  return { ...w, U, parent, child, globalNgo, globalNgo2, localNgo, villageRep, auditor, arbiter, driller, donor, visitor, nobody };
}

async function passMilestone(ev, id, signers) {
  for (const s of signers) await ev.connect(s).attest(id, hash(`ok-${id}`), "");
  await time.increase(WINDOW);
  await ev.release(id);
}

/// parent -> child (800) -> driller (600), then report the well.
async function wellBuilt() {
  const f = await world();
  const { parent, child, U, globalNgo, localNgo, villageRep, auditor, driller, impact } = f;
  await parent.connect(globalNgo).proposeMilestone("ipfs://grant", await child.getAddress(), U, usd(800));
  await passMilestone(parent, 0, [villageRep, auditor]);
  await child.connect(localNgo).proposeMilestone("ipfs://drill", driller.address, U, usd(600));
  await passMilestone(child, 0, [villageRep, auditor]);
  await impact
    .connect(driller)
    .reportDeed(await child.getAddress(), 0, "water-well", LAT, LON, "ipfs://report", sha("report"), [sha("photo1")], [
      "ipfs://photo1",
    ]);
  const deedId = await impact.deedIds(0);
  return { ...f, deedId };
}

describe("IdentityRegistry", function () {
  it("verifies, then stops verifying after revocation or issuer removal", async function () {
    const { identity, issuer, localNgo, globalNgo, admin } = await loadFixture(world);
    expect(await identity.isVerified(localNgo.address)).to.equal(true);
    expect(await identity.orgOf(localNgo.address)).to.equal(org("kayes-water"));

    await identity.connect(issuer).revoke(localNgo.address);
    expect(await identity.isVerified(localNgo.address)).to.equal(false);
    expect(await identity.orgOf(localNgo.address)).to.equal(ethers.ZeroHash);

    await identity.connect(admin).setIssuer(issuer.address, false, "Demo Charity Register");
    expect(await identity.isVerified(globalNgo.address)).to.equal(false);
  });

  it("only trusted issuers can issue", async function () {
    const { identity, nobody } = await loadFixture(world);
    await expect(identity.connect(nobody).issue(nobody.address, org("me"), "Me", "")).to.be.revertedWithCustomError(
      identity,
      "NotIssuer"
    );
  });
});

describe("EventRegistry signer checks", function () {
  it("refuses unverified organizers and signers", async function () {
    const { registry, U, nobody, localNgo, auditor, arbiter, globalNgo } = await loadFixture(world);
    await expect(
      createEvent(registry, nobody, { arbiter: arbiter.address, attesters: [localNgo.address, auditor.address], window: 1, tokens: [U] })
    ).to.be.revertedWithCustomError(registry, "Unverified");
    await expect(
      createEvent(registry, globalNgo, { arbiter: arbiter.address, attesters: [localNgo.address, nobody.address], window: 1, tokens: [U] })
    ).to.be.revertedWithCustomError(registry, "Unverified");
  });

  it("refuses signers from the organizer's org or the same org twice", async function () {
    const { registry, U, globalNgo, globalNgo2, localNgo, arbiter } = await loadFixture(world);
    // Second wallet of the organizer's own org: a sock puppet.
    await expect(
      createEvent(registry, globalNgo, { arbiter: arbiter.address, attesters: [localNgo.address, globalNgo2.address], window: 1, tokens: [U] })
    ).to.be.revertedWithCustomError(registry, "SameOrganization");
    // Two wallets of one org count once.
    await expect(
      createEvent(registry, localNgo, { arbiter: arbiter.address, attesters: [globalNgo.address, globalNgo2.address], window: 1, tokens: [U] })
    ).to.be.revertedWithCustomError(registry, "SameOrganization");
  });
});

describe("Multi-org flows", function () {
  it("a milestone paying a child event lands in the child's books", async function () {
    const { parent, child, U, globalNgo, villageRep, auditor, usdc } = await loadFixture(world);
    await parent.connect(globalNgo).proposeMilestone("ipfs://grant", await child.getAddress(), U, usd(800));
    await passMilestone(parent, 0, [villageRep, auditor]);
    expect(await child.totalDonated(U)).to.equal(usd(800));
    expect(await child.donatedBy(U, await parent.getAddress())).to.equal(usd(800));
    expect(await usdc.balanceOf(await child.getAddress())).to.equal(usd(800));
  });

  it("the child's organizer can't sign a grant into their own event", async function () {
    const { parent, child, U, globalNgo, localNgo } = await loadFixture(world);
    await parent.connect(globalNgo).proposeMilestone("ipfs://grant", await child.getAddress(), U, usd(800));
    await expect(parent.connect(localNgo).attest(0, hash("me"), "")).to.be.revertedWithCustomError(
      parent,
      "ConflictOfInterest"
    );
  });

  it("a cancelled child's leftovers can be pulled back to the parent pool", async function () {
    const f = await loadFixture(wellBuilt);
    const { parent, child, U, arbiter } = f;
    await child.connect(arbiter).cancel("ipfs://collective-dissolved");
    await expect(parent.reclaimFromChild(0))
      .to.emit(parent, "ReclaimedFromChild")
      .withArgs(0, await child.getAddress(), U, usd(200));
    expect(await parent.released(U)).to.equal(usd(600));
    expect(await parent.committed(U)).to.equal(usd(600));
  });
});

describe("ImpactRegistry", function () {
  it("records a deed tied to a paid milestone", async function () {
    const { impact, deedId, child, driller } = await loadFixture(wellBuilt);
    const d = await impact.deed(deedId);
    expect(d.eventAddr).to.equal(await child.getAddress());
    expect(d.reporter).to.equal(driller.address);
    expect(await impact.photoClaim(sha("photo1"))).to.equal(deedId);
  });

  it("refuses deeds for unpaid milestones and from outsiders", async function () {
    const { impact, child, localNgo, driller, nobody, U } = await loadFixture(wellBuilt);
    await child.connect(localNgo).proposeMilestone("ipfs://pump", driller.address, U, usd(100));
    const c = await child.getAddress();
    await expect(
      impact.connect(driller).reportDeed(c, 1, "water-well", 0, 0, "", sha("r"), [sha("p")], [""])
    ).to.be.revertedWithCustomError(impact, "MilestoneNotPaid");
    await expect(
      impact.connect(nobody).reportDeed(c, 0, "water-well", 0, 0, "", sha("r"), [sha("p")], [""])
    ).to.be.revertedWithCustomError(impact, "NotReporter");
  });

  it("blocks a second claim on the same well, even from another event, a few metres away", async function () {
    const f = await loadFixture(wellBuilt);
    const { registry, impact, U, globalNgo, localNgo, auditor, arbiter, driller, usdc, donor, deedId } = f;

    // Another org runs its own event and tries to claim the same well.
    const other = await createEvent(registry, globalNgo, {
      name: "Other",
      arbiter: arbiter.address,
      attesters: [localNgo.address, auditor.address],
      window: WINDOW,
      tokens: [U],
    });
    await usdc.mint(donor.address, usd(10));
    await usdc.connect(donor).approve(await other.getAddress(), usd(10));
    await other.connect(donor).donate(U, usd(10));
    await other.connect(globalNgo).proposeMilestone("ipfs://x", driller.address, U, usd(10));
    await passMilestone(other, 0, [localNgo, auditor]);

    const o = await other.getAddress();
    // ~16 m north-east: a neighbouring map cell, still blocked.
    await expect(
      impact.connect(driller).reportDeed(o, 0, "water-well", LAT + 15, LON + 15, "", sha("r2"), [sha("new")], [""])
    )
      .to.be.revertedWithCustomError(impact, "DeedAlreadyClaimed")
      .withArgs(deedId);

    // Reusing a photo of the first well elsewhere: blocked.
    await expect(
      impact.connect(driller).reportDeed(o, 0, "water-well", LAT + 5000, LON, "", sha("r2"), [sha("photo1")], [""])
    )
      .to.be.revertedWithCustomError(impact, "PhotoAlreadyUsed")
      .withArgs(sha("photo1"), deedId);

    // A different category at the same spot is a different deed.
    await impact.connect(driller).reportDeed(o, 0, "latrine", LAT, LON, "", sha("r3"), [sha("latrine")], [""]);
    expect(await impact.deedCount()).to.equal(2);
  });

  it("tallies public confirmations, verified ones separately, one vote each", async function () {
    const { impact, deedId, visitor, auditor, driller, nobody } = await loadFixture(wellBuilt);
    await impact.connect(visitor).vote(deedId, true, "ipfs://i-drank-from-it");
    await impact.connect(auditor).vote(deedId, true, "ipfs://site-visit");
    await impact.connect(nobody).vote(deedId, false, "ipfs://its-dry");
    const d = await impact.deed(deedId);
    expect(d.confirmations).to.equal(2);
    expect(d.verifiedConfirmations).to.equal(1);
    expect(d.disputes).to.equal(1);

    await expect(impact.connect(visitor).vote(deedId, true, "")).to.be.revertedWithCustomError(impact, "AlreadyVoted");
    await expect(impact.connect(driller).vote(deedId, true, "")).to.be.revertedWithCustomError(
      impact,
      "CannotVoteOwnDeed"
    );
  });
});
