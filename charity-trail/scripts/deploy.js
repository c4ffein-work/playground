// Deploys the EventRegistry and, optionally, one demo event.
//   PRIVATE_KEY=0x... npx hardhat run scripts/deploy.js --network baseSepolia
// Optional env to also create an event:
//   ARBITER=0x... ATTESTERS=0xa,0xb,0xc THRESHOLD=2 WINDOW_SECONDS=259200
const { ethers, network } = require("hardhat");

// Circle's test stablecoins on Base Sepolia. Double-check against
// https://developers.circle.com/stablecoins/usdc-contract-addresses before use.
const TOKENS = {
  baseSepolia: {
    USDC: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    EURC: "0x808456652fdb597867f38412077A9182bf77359F",
  },
};

async function main() {
  const [deployer] = await ethers.getSigners();
  console.log(`network ${network.name}, deployer ${deployer.address}`);

  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy();
  await registry.waitForDeployment();
  console.log(`EventRegistry: ${await registry.getAddress()}`);

  const { ARBITER, ATTESTERS, THRESHOLD = "2", WINDOW_SECONDS = "259200" } = process.env;
  if (!ARBITER || !ATTESTERS) return;

  const tokens = TOKENS[network.name];
  if (!tokens) throw new Error(`No stablecoin addresses configured for ${network.name}`);
  const tx = await registry.createEvent(
    "Demo event",
    "ipfs://demo",
    ARBITER,
    ATTESTERS.split(","),
    Number(THRESHOLD),
    Number(WINDOW_SECONDS),
    Object.values(tokens)
  );
  const log = (await tx.wait()).logs.map((l) => registry.interface.parseLog(l)).find((l) => l?.name === "EventCreated");
  console.log(`CharityEvent: ${log.args.eventAddress}`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
