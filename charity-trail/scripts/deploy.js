// Deploys every contract, and makes the deployer a trusted issuer so
// you can verify test organizations right away.
//   PRIVATE_KEY=0x... npx hardhat run scripts/deploy.js --network baseSepolia
const { ethers, network } = require("hardhat");

// Circle's test stablecoins on Base Sepolia, to whitelist when creating events.
// Double-check against https://developers.circle.com/stablecoins/usdc-contract-addresses
const TOKENS = {
  baseSepolia: {
    USDC: "0x036CbD53842c5426634e7929541eC2318f3dCF7e",
    EURC: "0x808456652fdb597867f38412077A9182bf77359F",
  },
};

async function main() {
  const [deployer] = await ethers.getSigners();
  console.log(`network ${network.name}, deployer ${deployer.address}`);

  const identity = await (await ethers.getContractFactory("IdentityRegistry")).deploy();
  await identity.waitForDeployment();
  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy(await identity.getAddress());
  await registry.waitForDeployment();
  const R = await registry.getAddress();
  const impact = await (await ethers.getContractFactory("ImpactRegistry")).deploy(R);
  await impact.waitForDeployment();
  const vaults = await (await ethers.getContractFactory("GivingVaults")).deploy(R);
  await vaults.waitForDeployment();
  const forum = await (await ethers.getContractFactory("Forum")).deploy();
  await forum.waitForDeployment();
  await (await identity.setIssuer(deployer.address, true, "Testnet issuer")).wait();

  console.log(`IdentityRegistry: ${await identity.getAddress()}`);
  console.log(`EventRegistry:    ${await registry.getAddress()}`);
  console.log(`ImpactRegistry:   ${await impact.getAddress()}`);
  console.log(`GivingVaults:     ${await vaults.getAddress()}`);
  console.log(`Forum:            ${await forum.getAddress()}`);
  console.log(`Stablecoins:      ${JSON.stringify(TOKENS[network.name] || {})}`);
  console.log("Next: identity.issue(wallet, orgId, name, uri) for each org, then registry.createEvent(...) from the organizer.");
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
