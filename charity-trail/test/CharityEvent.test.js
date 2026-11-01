const { expect } = require("chai");
const { ethers } = require("hardhat");
const { loadFixture } = require("@nomicfoundation/hardhat-network-helpers");
const { usd, hash, deployWorld, createEvent } = require("./helpers");

async function deployFixture() {
  const [, , organizer, ngo, partner, auditor, donor1, donor2, payee, stranger] = await ethers.getSigners();
  const world = await deployWorld({
    Organizer: [organizer, "org-main"],
    NGO: [ngo, "org-ngo"],
    Partner: [partner, "org-partner"],
    Auditor: [auditor, "org-auditor"],
  });
  const { registry, usdc, eurc } = world;
  const event = await createEvent(registry, organizer, {
    name: "Flood relief",
    attesters: [ngo.address, partner.address, auditor.address],
    tokens: [await usdc.getAddress(), await eurc.getAddress()],
  });
  for (const d of [donor1, donor2]) {
    for (const t of [usdc, eurc]) {
      await t.mint(d.address, usd(10_000));
      await t.connect(d).approve(await event.getAddress(), ethers.MaxUint256);
    }
  }
  return { ...world, event, organizer, ngo, partner, auditor, donor1, donor2, payee, stranger };
}

async function fundedWithMilestone() {
  const f = await deployFixture();
  const { event, usdc, organizer, donor1, donor2, payee } = f;
  const U = await usdc.getAddress();
  await event.connect(donor1).donate(U, usd(3000));
  await event.connect(donor2).donate(U, usd(1000));
  await event.connect(organizer).proposeMilestone("ipfs://water-pumps", payee.address, U, usd(2500));
  return { ...f, U };
}

describe("EventRegistry", function () {
  it("indexes created events", async function () {
    const { registry, event, organizer } = await loadFixture(deployFixture);
    expect(await registry.eventCount()).to.equal(1);
    expect(await registry.isEvent(await event.getAddress())).to.equal(true);
    expect(await event.organizer()).to.equal(organizer.address);
  });
});

describe("CharityEvent", function () {
  it("forbids the organizer from being a signer and a threshold above the signer count", async function () {
    const { usdc, organizer, ngo } = await loadFixture(deployFixture);
    const E = await ethers.getContractFactory("CharityEvent");
    const U = await usdc.getAddress();
    await expect(E.deploy("x", "", organizer.address, [ngo.address, organizer.address], 1, [U])).to.be.revertedWithCustomError(E, "BadConfig");
    await expect(E.deploy("x", "", organizer.address, [ngo.address], 2, [U])).to.be.revertedWithCustomError(E, "BadConfig");
  });

  it("accepts several stablecoins, and credits the real donor when someone pays for them", async function () {
    const { event, usdc, eurc, donor1, donor2 } = await loadFixture(deployFixture);
    const U = await usdc.getAddress();
    await event.connect(donor1).donate(U, usd(100));
    await event.connect(donor1).donate(await eurc.getAddress(), usd(50));
    await expect(event.connect(donor2).donateFor(donor1.address, U, usd(10)))
      .to.emit(event, "Donated")
      .withArgs(donor1.address, U, usd(10), donor2.address);
    expect(await event.donatedBy(U, donor1.address)).to.equal(usd(110));
    expect(await event.totalDonated(await eurc.getAddress())).to.equal(usd(50));
  });

  it("can't earmark more than was donated", async function () {
    const { event, organizer, payee, U } = await loadFixture(fundedWithMilestone);
    await expect(event.connect(organizer).proposeMilestone("x", payee.address, U, usd(1501))).to.be.revertedWithCustomError(
      event,
      "InsufficientUncommittedFunds"
    );
  });

  it("pays out with the signature that reaches the threshold", async function () {
    const { event, usdc, ngo, auditor, payee } = await loadFixture(fundedWithMilestone);
    await event.connect(ngo).attest(0, hash("invoice"), "ipfs://invoice");
    expect(await usdc.balanceOf(payee.address)).to.equal(0);
    await expect(event.connect(auditor).attest(0, hash("visit"), "ipfs://visit")).to.emit(event, "Released");
    expect(await usdc.balanceOf(payee.address)).to.equal(usd(2500));
    expect((await event.milestone(0)).status).to.equal(1); // Released
  });

  it("blocks outsiders, double signing and the payee signing for themselves", async function () {
    const { event, organizer, ngo, partner, stranger, U } = await loadFixture(fundedWithMilestone);
    await expect(event.connect(stranger).attest(0, hash("x"), "")).to.be.revertedWithCustomError(event, "NotAttester");
    await event.connect(ngo).attest(0, hash("x"), "");
    await expect(event.connect(ngo).attest(0, hash("y"), "")).to.be.revertedWithCustomError(event, "AlreadyAttested");
    await event.connect(organizer).proposeMilestone("fee", partner.address, U, 1);
    await expect(event.connect(partner).attest(1, hash("me"), "")).to.be.revertedWithCustomError(event, "ConflictOfInterest");
  });

  it("lets the organizer withdraw a milestone nobody will sign, freeing the money", async function () {
    const { event, organizer, ngo, U } = await loadFixture(fundedWithMilestone);
    await event.connect(organizer).withdrawMilestone(0, "ipfs://overpriced");
    expect(await event.committed(U)).to.equal(0);
    await expect(event.connect(ngo).attest(0, hash("x"), "")).to.be.revertedWithCustomError(event, "BadStatus");
  });

  it("when closed, refunds unspent money to donors pro rata", async function () {
    const { event, usdc, ngo, auditor, organizer, donor1, donor2, U } = await loadFixture(fundedWithMilestone);
    await event.connect(ngo).attest(0, hash("a"), "");
    await event.connect(auditor).attest(0, hash("b"), "");
    await event.connect(organizer).close("ipfs://done");
    expect(await event.refundableOf(U, donor1.address)).to.equal(usd(1125));
    await event.connect(donor1).claimRefund(U);
    await expect(event.connect(donor1).claimRefund(U)).to.be.revertedWithCustomError(event, "NothingToRefund");
    await event.connect(donor2).claimRefund(U);
    expect(await usdc.balanceOf(await event.getAddress())).to.equal(0);
    await expect(event.connect(donor1).donate(U, 1)).to.be.revertedWithCustomError(event, "EventClosed");
  });
});
