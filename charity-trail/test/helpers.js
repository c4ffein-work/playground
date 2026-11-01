const { ethers } = require("hardhat");

const usd = (n) => ethers.parseUnits(String(n), 6);
const hash = (s) => ethers.keccak256(ethers.toUtf8Bytes(s));
const sha = (s) => ethers.sha256(ethers.toUtf8Bytes(s));
const org = (s) => ethers.id(s);

/// Deploys identity + event + impact registries and two stablecoins, and
/// verifies `verified` signers (name => [signer, orgLabel]).
async function deployWorld(verified) {
  const [admin, issuer] = await ethers.getSigners();
  const identity = await (await ethers.getContractFactory("IdentityRegistry")).deploy();
  await identity.setIssuer(issuer.address, true, "Demo Charity Register");
  for (const [name, [signer, label]] of Object.entries(verified)) {
    await identity.connect(issuer).issue(signer.address, org(label), name, `https://register.example/${label}`);
  }
  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy(await identity.getAddress());
  const impact = await (await ethers.getContractFactory("ImpactRegistry")).deploy(
    await registry.getAddress(),
    await identity.getAddress()
  );
  const Token = await ethers.getContractFactory("MockStablecoin");
  const usdc = await Token.deploy("USD Coin", "USDC");
  const eurc = await Token.deploy("Euro Coin", "EURC");
  return { admin, issuer, identity, registry, impact, usdc, eurc };
}

async function createEvent(registry, organizer, { name = "Event", arbiter, attesters, threshold = 2, window, tokens }) {
  const tx = await registry
    .connect(organizer)
    .createEvent(name, `ipfs://${name}`, arbiter, attesters, threshold, window, tokens);
  const receipt = await tx.wait();
  const log = receipt.logs.map((l) => registry.interface.parseLog(l)).find((l) => l?.name === "EventCreated");
  return ethers.getContractAt("CharityEvent", log.args.eventAddress);
}

module.exports = { usd, hash, sha, org, deployWorld, createEvent };
