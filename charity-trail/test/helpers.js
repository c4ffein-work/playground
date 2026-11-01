const { ethers } = require("hardhat");

const usd = (n) => ethers.parseUnits(String(n), 6);
const hash = (s) => ethers.keccak256(ethers.toUtf8Bytes(s));
const sha = (s) => ethers.sha256(ethers.toUtf8Bytes(s));
const org = (s) => ethers.id(s);

/// Deploys every contract and two stablecoins, and verifies `verified`
/// signers (display name => [signer, org label]).
async function deployWorld(verified) {
  const [admin, issuer] = await ethers.getSigners();
  const identity = await (await ethers.getContractFactory("IdentityRegistry")).deploy();
  await identity.setIssuer(issuer.address, true, "Demo Charity Register");
  for (const [name, [signer, label]] of Object.entries(verified)) {
    await identity.connect(issuer).issue(signer.address, org(label), name, `https://register.example/${label}`);
  }
  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy(await identity.getAddress());
  const R = await registry.getAddress();
  const impact = await (await ethers.getContractFactory("ImpactRegistry")).deploy(R);
  const vaults = await (await ethers.getContractFactory("GivingVaults")).deploy(R);
  const forum = await (await ethers.getContractFactory("Forum")).deploy();
  const Token = await ethers.getContractFactory("MockStablecoin");
  const usdc = await Token.deploy("USD Coin", "USDC");
  const eurc = await Token.deploy("Euro Coin", "EURC");
  return { admin, issuer, identity, registry, impact, vaults, forum, usdc, eurc };
}

async function createEvent(registry, organizer, { name = "Event", attesters, threshold = 2, tokens }) {
  const tx = await registry.connect(organizer).createEvent(name, `ipfs://${name}`, attesters, threshold, tokens);
  const receipt = await tx.wait();
  const log = receipt.logs.map((l) => registry.interface.parseLog(l)).find((l) => l?.name === "EventCreated");
  return ethers.getContractAt("CharityEvent", log.args.eventAddress);
}

module.exports = { usd, hash, sha, org, deployWorld, createEvent };
