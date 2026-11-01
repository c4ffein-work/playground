const { expect } = require("chai");
const { ethers } = require("hardhat");
const { loadFixture, time } = require("@nomicfoundation/hardhat-network-helpers");

const usd = (n) => ethers.parseUnits(String(n), 6);
const hash = (s) => ethers.keccak256(ethers.toUtf8Bytes(s));
const WINDOW = 3 * 24 * 3600; // 3 days

async function deployFixture() {
  const [organizer, arbiter, ngo, partner, auditor, donor1, donor2, payee, stranger] = await ethers.getSigners();

  const Token = await ethers.getContractFactory("MockStablecoin");
  const usdc = await Token.deploy("USD Coin", "USDC");
  const eurc = await Token.deploy("Euro Coin", "EURC");

  const Registry = await ethers.getContractFactory("EventRegistry");
  const registry = await Registry.deploy();

  const tx = await registry
    .connect(organizer)
    .createEvent(
      "Flood relief - Region X",
      "ipfs://event-metadata",
      arbiter.address,
      [ngo.address, partner.address, auditor.address],
      2,
      WINDOW,
      [await usdc.getAddress(), await eurc.getAddress()]
    );
  const receipt = await tx.wait();
  const created = receipt.logs.map((l) => registry.interface.parseLog(l)).find((l) => l?.name === "EventCreated");
  const event = await ethers.getContractAt("CharityEvent", created.args.eventAddress);

  for (const d of [donor1, donor2]) {
    for (const t of [usdc, eurc]) {
      await t.mint(d.address, usd(10_000));
      await t.connect(d).approve(await event.getAddress(), ethers.MaxUint256);
    }
  }

  return { registry, event, usdc, eurc, organizer, arbiter, ngo, partner, auditor, donor1, donor2, payee, stranger };
}

async function fundedWithMilestone() {
  const f = await deployFixture();
  const { event, usdc, organizer, donor1, donor2, payee } = f;
  await event.connect(donor1).donate(await usdc.getAddress(), usd(3000));
  await event.connect(donor2).donate(await usdc.getAddress(), usd(1000));
  await event.connect(organizer).proposeMilestone("ipfs://water-pumps", payee.address, await usdc.getAddress(), usd(2500));
  return f;
}

describe("EventRegistry", function () {
  it("indexes created events", async function () {
    const { registry, event, organizer } = await loadFixture(deployFixture);
    expect(await registry.eventCount()).to.equal(1);
    expect(await registry.events(0)).to.equal(await event.getAddress());
    expect(await event.organizer()).to.equal(organizer.address);
  });
});

describe("CharityEvent", function () {
  describe("setup", function () {
    it("rejects a threshold above the number of attesters", async function () {
      const { usdc, organizer, arbiter, ngo } = await loadFixture(deployFixture);
      const E = await ethers.getContractFactory("CharityEvent");
      await expect(
        E.deploy("x", "", organizer.address, arbiter.address, [ngo.address], 2, WINDOW, [await usdc.getAddress()])
      ).to.be.revertedWithCustomError(E, "BadConfig");
    });

    it("forbids the organizer from being an attester", async function () {
      const { usdc, organizer, arbiter, ngo } = await loadFixture(deployFixture);
      const E = await ethers.getContractFactory("CharityEvent");
      await expect(
        E.deploy("x", "", organizer.address, arbiter.address, [ngo.address, organizer.address], 1, WINDOW, [
          await usdc.getAddress(),
        ])
      ).to.be.revertedWithCustomError(E, "BadConfig");
    });
  });

  describe("donations", function () {
    it("accepts several stablecoins and tracks them separately", async function () {
      const { event, usdc, eurc, donor1 } = await loadFixture(deployFixture);
      await expect(event.connect(donor1).donate(await usdc.getAddress(), usd(100)))
        .to.emit(event, "Donated")
        .withArgs(donor1.address, await usdc.getAddress(), usd(100));
      await event.connect(donor1).donate(await eurc.getAddress(), usd(50));
      expect(await event.totalDonated(await usdc.getAddress())).to.equal(usd(100));
      expect(await event.totalDonated(await eurc.getAddress())).to.equal(usd(50));
    });

    it("rejects tokens that aren't whitelisted", async function () {
      const { event, donor1 } = await loadFixture(deployFixture);
      const Token = await ethers.getContractFactory("MockStablecoin");
      const rogue = await Token.deploy("Rogue", "RGE");
      await expect(event.connect(donor1).donate(await rogue.getAddress(), 1)).to.be.revertedWithCustomError(
        event,
        "TokenNotAccepted"
      );
    });
  });

  describe("milestones", function () {
    it("can't earmark more than was donated", async function () {
      const { event, usdc, organizer, payee } = await loadFixture(fundedWithMilestone);
      // 4000 donated, 2500 already committed.
      await expect(
        event.connect(organizer).proposeMilestone("ipfs://too-much", payee.address, await usdc.getAddress(), usd(1501))
      ).to.be.revertedWithCustomError(event, "InsufficientUncommittedFunds");
    });

    it("only the organizer proposes", async function () {
      const { event, usdc, stranger, payee } = await loadFixture(fundedWithMilestone);
      await expect(
        event.connect(stranger).proposeMilestone("ipfs://x", payee.address, await usdc.getAddress(), 1)
      ).to.be.revertedWithCustomError(event, "NotOrganizer");
    });

    it("pays out after threshold attestations and an undisputed window", async function () {
      const { event, usdc, ngo, auditor, payee, stranger } = await loadFixture(fundedWithMilestone);

      await event.connect(ngo).attest(0, hash("invoice"), "ipfs://invoice");
      expect((await event.milestone(0)).status).to.equal(0); // Proposed

      await expect(event.connect(auditor).attest(0, hash("site-visit"), "ipfs://site-visit")).to.emit(
        event,
        "MilestoneApproved"
      );
      expect((await event.milestone(0)).status).to.equal(1); // Approved

      await expect(event.connect(stranger).release(0)).to.be.revertedWithCustomError(event, "ChallengeWindowOpen");

      await time.increase(WINDOW);
      await expect(event.connect(stranger).release(0))
        .to.emit(event, "Released")
        .withArgs(0, payee.address, await usdc.getAddress(), usd(2500));
      expect(await usdc.balanceOf(payee.address)).to.equal(usd(2500));
      expect((await event.milestone(0)).status).to.equal(3); // Released
    });

    it("blocks non-attesters, double attestation and self-attestation by the payee", async function () {
      const { event, usdc, organizer, ngo, partner, stranger } = await loadFixture(fundedWithMilestone);
      await expect(event.connect(stranger).attest(0, hash("x"), "")).to.be.revertedWithCustomError(
        event,
        "NotAttester"
      );
      await event.connect(ngo).attest(0, hash("x"), "");
      await expect(event.connect(ngo).attest(0, hash("y"), "")).to.be.revertedWithCustomError(
        event,
        "AlreadyAttested"
      );

      // The partner is an attester; a milestone paying the partner can't be signed by them.
      await event.connect(organizer).proposeMilestone("ipfs://partner-fee", partner.address, await usdc.getAddress(), 1);
      await expect(event.connect(partner).attest(1, hash("me"), "")).to.be.revertedWithCustomError(
        event,
        "ConflictOfInterest"
      );
    });

    it("revoking an attestation drops an approved milestone back to proposed", async function () {
      const { event, ngo, auditor } = await loadFixture(fundedWithMilestone);
      await event.connect(ngo).attest(0, hash("a"), "");
      await event.connect(auditor).attest(0, hash("b"), "");
      await event.connect(auditor).revokeAttestation(0, "ipfs://changed-my-mind");
      const m = await event.milestone(0);
      expect(m.status).to.equal(0);
      expect(m.approvals).to.equal(1);
    });
  });

  describe("disputes", function () {
    async function approved() {
      const f = await fundedWithMilestone();
      await f.event.connect(f.ngo).attest(0, hash("a"), "");
      await f.event.connect(f.auditor).attest(0, hash("b"), "");
      return f;
    }

    it("a donor can freeze a milestone and the arbiter can reject it", async function () {
      const { event, usdc, donor2, arbiter } = await loadFixture(approved);
      await expect(event.connect(donor2).challenge(0, "ipfs://pumps-never-arrived"))
        .to.emit(event, "Challenged")
        .withArgs(0, donor2.address, "ipfs://pumps-never-arrived");

      await time.increase(WINDOW);
      await expect(event.release(0)).to.be.revertedWithCustomError(event, "BadStatus");

      await event.connect(arbiter).resolveDispute(0, false, "ipfs://ruling");
      expect((await event.milestone(0)).status).to.equal(4); // Rejected
      expect(await event.committed(await usdc.getAddress())).to.equal(0);
    });

    it("the arbiter can uphold a challenged milestone and pay it", async function () {
      const { event, usdc, donor1, arbiter, payee } = await loadFixture(approved);
      await event.connect(donor1).challenge(0, "ipfs://doubt");
      await event.connect(arbiter).resolveDispute(0, true, "ipfs://checked-ok");
      expect(await usdc.balanceOf(payee.address)).to.equal(usd(2500));
    });

    it("strangers can't challenge, and nobody can after the window", async function () {
      const { event, donor1, stranger } = await loadFixture(approved);
      await expect(event.connect(stranger).challenge(0, "")).to.be.revertedWithCustomError(
        event,
        "NotAllowedToChallenge"
      );
      await time.increase(WINDOW);
      await expect(event.connect(donor1).challenge(0, "")).to.be.revertedWithCustomError(
        event,
        "ChallengeWindowClosed"
      );
    });
  });

  describe("cancel and refund", function () {
    it("refunds unreleased funds to donors pro rata", async function () {
      const { event, usdc, ngo, auditor, arbiter, donor1, donor2 } = await loadFixture(fundedWithMilestone);
      const usdcAddr = await usdc.getAddress();

      // Release 2500 of 4000, then cancel: 1500 left, split 3:1.
      await event.connect(ngo).attest(0, hash("a"), "");
      await event.connect(auditor).attest(0, hash("b"), "");
      await time.increase(WINDOW);
      await event.release(0);

      await event.connect(arbiter).cancel("ipfs://organizer-vanished");
      expect(await event.refundableOf(usdcAddr, donor1.address)).to.equal(usd(1125));
      expect(await event.refundableOf(usdcAddr, donor2.address)).to.equal(usd(375));

      await expect(event.connect(donor1).claimRefund(usdcAddr))
        .to.emit(event, "Refunded")
        .withArgs(donor1.address, usdcAddr, usd(1125));
      await expect(event.connect(donor1).claimRefund(usdcAddr)).to.be.revertedWithCustomError(
        event,
        "NothingToRefund"
      );
      await event.connect(donor2).claimRefund(usdcAddr);
      expect(await usdc.balanceOf(await event.getAddress())).to.equal(0);
    });

    it("stops donations and releases once cancelled", async function () {
      const { event, usdc, ngo, auditor, arbiter, donor1 } = await loadFixture(fundedWithMilestone);
      await event.connect(ngo).attest(0, hash("a"), "");
      await event.connect(auditor).attest(0, hash("b"), "");
      await event.connect(arbiter).cancel("");
      await time.increase(WINDOW);
      await expect(event.release(0)).to.be.revertedWithCustomError(event, "EventCancelled");
      await expect(event.connect(donor1).donate(await usdc.getAddress(), 1)).to.be.revertedWithCustomError(
        event,
        "EventCancelled"
      );
    });
  });
});
