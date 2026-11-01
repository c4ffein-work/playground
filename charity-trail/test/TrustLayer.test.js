const { expect } = require("chai");
const { ethers } = require("hardhat");
const { loadFixture, time } = require("@nomicfoundation/hardhat-network-helpers");
const { usd, hash, sha, org, deployWorld, createEvent } = require("./helpers");

// Kayes region, Mali (demo coordinates), degrees * 1e5.
const LAT = 1445012;
const LON = -1144031;
const MONTH = 28 * 24 * 3600;

async function world() {
  const s = await ethers.getSigners();
  const [, , globalNgo, globalNgo2, localNgo, villageRep, auditor, curator, driller, donor, visitor, nobody] = s;
  const w = await deployWorld({
    "Blue Horizon Foundation": [globalNgo, "blue-horizon"],
    "Blue Horizon (2nd wallet)": [globalNgo2, "blue-horizon"],
    "Kayes Water Collective": [localNgo, "kayes-water"],
    "Village council": [villageRep, "village-council"],
    "Sahel Audit": [auditor, "sahel-audit"],
  });
  const U = await w.usdc.getAddress();
  const parent = await createEvent(w.registry, globalNgo, {
    name: "Clean water Sahel",
    attesters: [localNgo.address, auditor.address, villageRep.address],
    tokens: [U],
  });
  const child = await createEvent(w.registry, localNgo, {
    name: "Wells in Kayes",
    attesters: [villageRep.address, auditor.address],
    tokens: [U],
  });
  await w.usdc.mint(donor.address, usd(1000));
  await w.usdc.connect(donor).approve(await parent.getAddress(), ethers.MaxUint256);
  await parent.connect(donor).donate(U, usd(1000));
  return { ...w, U, parent, child, globalNgo, globalNgo2, localNgo, villageRep, auditor, curator, driller, donor, visitor, nobody };
}

/// parent -> child (800) -> driller (600), then report the well.
async function wellBuilt() {
  const f = await world();
  const { parent, child, U, globalNgo, localNgo, villageRep, auditor, driller, impact } = f;
  await parent.connect(globalNgo).proposeMilestone("grant", await child.getAddress(), U, usd(800));
  await parent.connect(villageRep).attest(0, hash("ok"), "");
  await parent.connect(auditor).attest(0, hash("ok"), "");
  await child.connect(localNgo).proposeMilestone("drill", driller.address, U, usd(600));
  await child.connect(villageRep).attest(0, hash("ok"), "");
  await child.connect(auditor).attest(0, hash("ok"), "");
  await impact
    .connect(driller)
    .reportDeed(await child.getAddress(), 0, "water-well", LAT, LON, false, "ipfs://report", sha("report"), [sha("photo1")], ["ipfs://photo1"]);
  return { ...f, deedId: await impact.deedIds(0) };
}

/// A second paid milestone, from another event, that a copycat can report.
async function withCopycat() {
  const f = await wellBuilt();
  const { registry, U, globalNgo, localNgo, auditor, driller, usdc, donor } = f;
  const other = await createEvent(registry, globalNgo, { name: "Other", attesters: [localNgo.address, auditor.address], tokens: [U] });
  await usdc.mint(donor.address, usd(10));
  await usdc.connect(donor).approve(await other.getAddress(), usd(10));
  await other.connect(donor).donate(U, usd(10));
  await other.connect(globalNgo).proposeMilestone("x", driller.address, U, usd(10));
  await other.connect(localNgo).attest(0, hash("ok"), "");
  await other.connect(auditor).attest(0, hash("ok"), "");
  return { ...f, other, o: await other.getAddress() };
}

describe("IdentityRegistry", function () {
  it("verifies, then stops verifying after revocation or issuer removal", async function () {
    const { identity, issuer, localNgo, globalNgo, admin } = await loadFixture(world);
    expect(await identity.orgOf(localNgo.address)).to.equal(org("kayes-water"));
    await identity.connect(issuer).revoke(localNgo.address);
    expect(await identity.isVerified(localNgo.address)).to.equal(false);
    await identity.connect(admin).setIssuer(issuer.address, false, "x");
    expect(await identity.isVerified(globalNgo.address)).to.equal(false);
  });

  it("only trusted issuers can issue", async function () {
    const { identity, nobody } = await loadFixture(world);
    await expect(identity.connect(nobody).issue(nobody.address, org("me"), "Me", "")).to.be.revertedWithCustomError(identity, "NotIssuer");
  });
});

describe("EventRegistry signer checks", function () {
  it("refuses unverified organizers and signers", async function () {
    const { registry, U, nobody, localNgo, auditor, globalNgo } = await loadFixture(world);
    await expect(createEvent(registry, nobody, { attesters: [localNgo.address, auditor.address], tokens: [U] })).to.be.revertedWithCustomError(registry, "Unverified");
    await expect(createEvent(registry, globalNgo, { attesters: [localNgo.address, nobody.address], tokens: [U] })).to.be.revertedWithCustomError(registry, "Unverified");
  });

  it("refuses signers from the organizer's org or the same org twice", async function () {
    const { registry, U, globalNgo, globalNgo2, localNgo } = await loadFixture(world);
    await expect(createEvent(registry, globalNgo, { attesters: [localNgo.address, globalNgo2.address], tokens: [U] })).to.be.revertedWithCustomError(registry, "SameOrganization");
    await expect(createEvent(registry, localNgo, { attesters: [globalNgo.address, globalNgo2.address], tokens: [U] })).to.be.revertedWithCustomError(registry, "SameOrganization");
  });
});

describe("Multi-org flows", function () {
  it("a grant into a child event lands in the child's books", async function () {
    const { child, U, parent, usdc } = await loadFixture(wellBuilt);
    expect(await child.donatedBy(U, await parent.getAddress())).to.equal(usd(800));
    expect(await usdc.balanceOf(await child.getAddress())).to.equal(usd(200));
  });

  it("the child's organizer can't sign a grant into their own event", async function () {
    const { parent, child, U, globalNgo, localNgo } = await loadFixture(world);
    await parent.connect(globalNgo).proposeMilestone("grant", await child.getAddress(), U, usd(800));
    await expect(parent.connect(localNgo).attest(0, hash("me"), "")).to.be.revertedWithCustomError(parent, "ConflictOfInterest");
  });

  it("a closed child's leftovers go back to the parent pool", async function () {
    const { parent, child, U, localNgo } = await loadFixture(wellBuilt);
    await child.connect(localNgo).close("ipfs://done");
    await expect(parent.reclaimFromChild(0)).to.emit(parent, "ReclaimedFromChild").withArgs(0, await child.getAddress(), U, usd(200));
    expect(await parent.released(U)).to.equal(usd(600));
  });
});

describe("ImpactRegistry", function () {
  it("records a deed tied to a paid milestone", async function () {
    const { impact, deedId, child, driller } = await loadFixture(wellBuilt);
    const d = await impact.deed(deedId);
    expect(d.eventAddr).to.equal(await child.getAddress());
    expect(d.reporter).to.equal(driller.address);
    expect(d.conflicts).to.equal(0);
  });

  it("refuses deeds for unpaid milestones, from outsiders, or twice per milestone", async function () {
    const { impact, child, localNgo, driller, nobody, U } = await loadFixture(wellBuilt);
    await child.connect(localNgo).proposeMilestone("pump", driller.address, U, usd(100));
    const c = await child.getAddress();
    const args = (id) => [c, id, "water-well", 0, 0, false, "", sha("r"), [sha("p")], [""]];
    await expect(impact.connect(driller).reportDeed(...args(1))).to.be.revertedWithCustomError(impact, "MilestoneNotPaid");
    await expect(impact.connect(nobody).reportDeed(...args(0))).to.be.revertedWithCustomError(impact, "NotReporter");
    await expect(impact.connect(driller).reportDeed(...args(0))).to.be.revertedWithCustomError(impact, "MilestoneAlreadyClaimed");
  });

  it("flags a claim ~16 m away as contested instead of refusing it", async function () {
    const { impact, driller, deedId, o } = await loadFixture(withCopycat);
    const copyId = ethers.keccak256(ethers.AbiCoder.defaultAbiCoder().encode(["address", "uint256"], [o, 0]));
    await expect(impact.connect(driller).reportDeed(o, 0, "water-well", LAT + 15, LON + 5, false, "", sha("r2"), [sha("new")], [""]))
      .to.emit(impact, "DeedContested")
      .withArgs(copyId, deedId, "place");
    expect((await impact.deed(deedId)).conflicts).to.equal(1);
    expect(await impact.deedCount()).to.equal(2);
  });

  it("flags a reused photo as contested", async function () {
    const { impact, driller, deedId, o } = await loadFixture(withCopycat);
    await expect(impact.connect(driller).reportDeed(o, 0, "water-well", LAT + 5000, LON, false, "", sha("r2"), [sha("photo1")], [""]))
      .to.emit(impact, "DeedContested");
    expect((await impact.deed(deedId)).conflicts).to.equal(1);
  });

  it("stores sensitive deeds at ~11 km precision and never matches them by place", async function () {
    const { impact, driller, o } = await loadFixture(withCopycat);
    await expect(impact.connect(driller).reportDeed(o, 0, "water-well", LAT, LON, true, "", sha("r2"), [sha("new")], [""])).to.not.emit(impact, "DeedContested");
    const d = await impact.deed(await impact.deedIds(1));
    expect(d.latE5).to.equal(1440000);
    expect(d.lonE5).to.equal(-1150000);
    expect(d.sensitive).to.equal(true);
  });
});

describe("GivingVaults", function () {
  async function vaultSetup() {
    const f = await world();
    const { vaults, usdc, donor, U } = f;
    await usdc.mint(donor.address, usd(1000));
    await usdc.connect(donor).approve(await vaults.getAddress(), ethers.MaxUint256);
    await vaults.connect(donor).deposit(U, usd(300));
    await vaults.connect(donor).setMonthly(U, usd(100));
    return f;
  }

  it("pays the monthly gift split by allocation, credited to the donor, once per month", async function () {
    const { vaults, parent, child, donor, U } = await loadFixture(vaultSetup);
    await vaults.connect(donor).setAllocation([await parent.getAddress(), await child.getAddress()], [7000, 3000]);
    await vaults.pay(donor.address, U);
    expect(await parent.donatedBy(U, donor.address)).to.equal(usd(1070)); // 1000 direct + 70
    expect(await child.donatedBy(U, donor.address)).to.equal(usd(30));
    await expect(vaults.pay(donor.address, U)).to.be.revertedWithCustomError(vaults, "NotDue");
    await time.increase(MONTH);
    await vaults.pay(donor.address, U);
    expect((await vaults.plans(donor.address, U)).balance).to.equal(usd(100));
  });

  it("redirecting applies from the next payment, and unspent money can be withdrawn", async function () {
    const { vaults, parent, child, donor, U, usdc } = await loadFixture(vaultSetup);
    await vaults.connect(donor).setAllocation([await parent.getAddress()], [10000]);
    await vaults.pay(donor.address, U);
    await vaults.connect(donor).setAllocation([await child.getAddress()], [10000]);
    await time.increase(MONTH);
    await vaults.pay(donor.address, U);
    expect(await child.donatedBy(U, donor.address)).to.equal(usd(100));
    const before = await usdc.balanceOf(donor.address);
    await vaults.connect(donor).withdraw(U, usd(100));
    expect(await usdc.balanceOf(donor.address)).to.equal(before + usd(100));
  });

  it("followers get the curator's split, including later changes", async function () {
    const { vaults, parent, child, donor, curator, U } = await loadFixture(vaultSetup);
    await vaults.connect(curator).setAllocation([await parent.getAddress()], [10000]);
    await vaults.connect(donor).follow(curator.address);
    await vaults.pay(donor.address, U);
    expect(await parent.donatedBy(U, donor.address)).to.equal(usd(1100));
    await vaults.connect(curator).setAllocation([await child.getAddress()], [10000]);
    await time.increase(MONTH);
    await vaults.pay(donor.address, U);
    expect(await child.donatedBy(U, donor.address)).to.equal(usd(100));
  });

  it("keeps the share of a closed event in the vault", async function () {
    const { vaults, parent, child, donor, localNgo, U } = await loadFixture(vaultSetup);
    await vaults.connect(donor).setAllocation([await parent.getAddress(), await child.getAddress()], [5000, 5000]);
    await child.connect(localNgo).close("");
    await expect(vaults.pay(donor.address, U)).to.emit(vaults, "Skipped");
    expect((await vaults.plans(donor.address, U)).balance).to.equal(usd(250));
  });

  it("only allows registered events, with shares summing to 100%", async function () {
    const { vaults, parent, donor, nobody } = await loadFixture(vaultSetup);
    await expect(vaults.connect(donor).setAllocation([nobody.address], [10000])).to.be.revertedWithCustomError(vaults, "BadAllocation");
    await expect(vaults.connect(donor).setAllocation([await parent.getAddress()], [9000])).to.be.revertedWithCustomError(vaults, "BadAllocation");
  });
});

describe("Forum", function () {
  it("records posts with a stance, and delegation", async function () {
    const { forum, visitor, auditor, deedId } = await loadFixture(wellBuilt);
    await expect(forum.connect(visitor).post(deedId, ethers.ZeroHash, 1, "ipfs://works", sha("works"))).to.emit(forum, "Posted");
    await expect(forum.connect(visitor).post(deedId, ethers.ZeroHash, 2, "", ethers.ZeroHash)).to.be.revertedWithCustomError(forum, "BadStance");
    await expect(forum.connect(visitor).delegate(auditor.address)).to.emit(forum, "Delegated").withArgs(visitor.address, auditor.address);
    await expect(forum.connect(visitor).delegate(visitor.address)).to.be.revertedWithCustomError(forum, "SelfDelegation");
  });
});
