// Rebuilds the public trail of a CharityEvent from its logs alone.
// Usage: EVENT=0x... npx hardhat run scripts/trail.js --network baseSepolia
const { ethers } = require("hardhat");

const short = (a) => `${a.slice(0, 6)}…${a.slice(-4)}`;

async function printTrail(event, labels = {}) {
  const who = (a) => labels[a] || short(a);
  const symbols = {};
  for (const t of await event.acceptedTokens()) {
    const erc = await ethers.getContractAt("IERC20Metadata", t);
    symbols[t] = { sym: await erc.symbol(), dec: await erc.decimals() };
  }
  const amt = (token, v) => `${ethers.formatUnits(v, symbols[token].dec)} ${symbols[token].sym}`;

  const logs = await event.queryFilter("*", 0, "latest");
  console.log(`\n=== Public trail: ${await event.name()} (${short(await event.getAddress())}) ===`);
  for (const log of logs) {
    const name = log.fragment?.name;
    const a = log.args;
    const line = {
      Donated: () => `donation   ${who(a.donor)} gave ${amt(a.token, a.amount)}`,
      MilestoneProposed: () =>
        `milestone  #${a.id} proposed: ${amt(a.token, a.amount)} -> ${who(a.payee)}  [${a.descriptionURI}]`,
      Attested: () => `attest     #${a.id} signed by ${who(a.attester)}  [${a.evidenceURI}]`,
      AttestationRevoked: () => `revoke     #${a.id} by ${who(a.attester)}  [${a.reasonURI}]`,
      MilestoneApproved: () =>
        `approved   #${a.id}, challenge window until ${new Date(Number(a.challengeDeadline) * 1000).toISOString()}`,
      Challenged: () => `CHALLENGE  #${a.id} by ${who(a.by)}  [${a.reasonURI}]`,
      DisputeResolved: () => `ruling     #${a.id} ${a.upheld ? "upheld" : "REJECTED"}  [${a.reasonURI}]`,
      Released: () => `PAID       #${a.id} ${amt(a.token, a.amount)} -> ${who(a.payee)}`,
      Cancelled: () => `CANCELLED  [${a.reasonURI}]`,
      Refunded: () => `refund     ${amt(a.token, a.amount)} -> ${who(a.donor)}`,
    }[name];
    if (line) console.log(`blk ${String(log.blockNumber).padStart(4)}  ${line()}`);
  }

  console.log("\n--- Balance sheet ---");
  for (const t of Object.keys(symbols)) {
    const [d, c, r] = await Promise.all([event.totalDonated(t), event.committed(t), event.released(t)]);
    console.log(
      `${symbols[t].sym.padEnd(5)} donated ${amt(t, d)} | earmarked ${amt(t, c - r)} | paid ${amt(t, r)} | free ${amt(t, d - c)}`
    );
  }
}

module.exports = { printTrail };

if (require.main === module) {
  if (!process.env.EVENT) throw new Error("Set EVENT=<CharityEvent address>");
  ethers
    .getContractAt("CharityEvent", process.env.EVENT)
    .then((e) => printTrail(e))
    .catch((err) => {
      console.error(err);
      process.exitCode = 1;
    });
}
