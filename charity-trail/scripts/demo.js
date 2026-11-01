// End-to-end story on the local Hardhat network: npx hardhat run scripts/demo.js
const { ethers, network } = require("hardhat");
const { printTrail } = require("./trail");

const usd = (n) => ethers.parseUnits(String(n), 6);
const hash = (s) => ethers.keccak256(ethers.toUtf8Bytes(s));
const WINDOW = 3 * 24 * 3600;
const skipWindow = async () => {
  await network.provider.send("evm_increaseTime", [WINDOW]);
  await network.provider.send("evm_mine");
};

async function main() {
  const [organizer, arbiter, localNgo, villageRep, auditor, alice, bob, pumpCo, truckCo] = await ethers.getSigners();
  const labels = Object.fromEntries(
    Object.entries({ organizer, arbiter, localNgo, villageRep, auditor, alice, bob, pumpCo, truckCo }).map(
      ([k, s]) => [s.address, k]
    )
  );

  const Token = await ethers.getContractFactory("MockStablecoin");
  const usdc = await Token.deploy("USD Coin", "USDC");
  const eurc = await Token.deploy("Euro Coin", "EURC");
  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy();

  const tx = await registry
    .connect(organizer)
    .createEvent(
      "Flood relief - Region X",
      "ipfs://event-charter",
      arbiter.address,
      [localNgo.address, villageRep.address, auditor.address],
      2,
      WINDOW,
      [await usdc.getAddress(), await eurc.getAddress()]
    );
  const log = (await tx.wait()).logs.map((l) => registry.interface.parseLog(l)).find((l) => l?.name === "EventCreated");
  const event = await ethers.getContractAt("CharityEvent", log.args.eventAddress);
  const ev = await event.getAddress();
  const U = await usdc.getAddress();
  const E = await eurc.getAddress();

  // Donations in two stablecoins.
  await usdc.mint(alice.address, usd(5000));
  await eurc.mint(bob.address, usd(3000));
  await usdc.connect(alice).approve(ev, usd(5000));
  await eurc.connect(bob).approve(ev, usd(3000));
  await event.connect(alice).donate(U, usd(5000));
  await event.connect(bob).donate(E, usd(3000));

  // Milestone 0: water pumps. Two stakeholders sign, window passes, paid.
  await event.connect(organizer).proposeMilestone("ipfs://budget/water-pumps", pumpCo.address, U, usd(3200));
  await event.connect(localNgo).attest(0, hash("pump invoice"), "ipfs://evidence/pump-invoice");
  await event.connect(villageRep).attest(0, hash("pumps installed"), "ipfs://evidence/pumps-photos");
  await skipWindow();
  await event.release(0);

  // Milestone 1: trucks. Signed, but a donor challenges and the arbiter rejects.
  await event.connect(organizer).proposeMilestone("ipfs://budget/truck-rental", truckCo.address, E, usd(2500));
  await event.connect(localNgo).attest(1, hash("truck quote"), "ipfs://evidence/truck-quote");
  await event.connect(auditor).attest(1, hash("truck check"), "ipfs://evidence/auditor-note");
  await event.connect(bob).challenge(1, "ipfs://challenge/quote-is-3x-market-price");
  await event.connect(arbiter).resolveDispute(1, false, "ipfs://ruling/overpriced");

  await printTrail(event, labels);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
