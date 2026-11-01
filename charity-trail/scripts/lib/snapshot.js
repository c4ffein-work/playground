// Reads every log from the three registries and their events, and builds
// one JSON snapshot for the tracker site. Nothing here is trusted state:
// it's all rebuilt from public chain data (plus the off-chain documents,
// which the site re-hashes against the on-chain hashes).
const { ethers } = require("hardhat");

const STATUS = ["Proposed", "Approved", "Disputed", "Released", "Rejected"];

async function buildSnapshot({ identity, registry, impact, labels = {}, docs = {}, blocked = [], meta = {} }) {
  const provider = ethers.provider;
  const blockTimes = {};
  const timeOf = async (bn) => {
    if (!(bn in blockTimes)) blockTimes[bn] = (await provider.getBlock(bn)).timestamp;
    return blockTimes[bn];
  };
  const stamp = async (log) => ({ time: await timeOf(log.blockNumber), block: log.blockNumber, tx: log.transactionHash });

  // ---- identity
  const issuers = {};
  for (const l of await identity.queryFilter(identity.filters.IssuerSet())) {
    issuers[l.args.issuer] = { name: l.args.name, active: l.args.active };
  }
  const orgs = {};
  for (const l of await identity.queryFilter(identity.filters.CredentialIssued())) {
    const a = l.args.subject;
    orgs[a] = {
      name: l.args.name,
      orgId: l.args.orgId,
      uri: l.args.uri,
      issuer: l.args.issuer,
      issuedAt: await timeOf(l.blockNumber),
      verified: await identity.isVerified(a),
    };
  }

  // ---- tokens & events
  const tokens = {};
  const tokenInfo = async (t) => {
    if (!tokens[t]) {
      const erc = await ethers.getContractAt("IERC20Metadata", t);
      tokens[t] = { symbol: await erc.symbol(), decimals: Number(await erc.decimals()) };
    }
    return tokens[t];
  };
  const units = (t, v) => Number(ethers.formatUnits(v, tokens[t].decimals));

  const events = [];
  for (const l of await registry.queryFilter(registry.filters.EventCreated())) {
    const ev = await ethers.getContractAt("CharityEvent", l.args.eventAddress);
    const address = l.args.eventAddress;
    const accepted = await ev.acceptedTokens();
    for (const t of accepted) await tokenInfo(t);

    const e = {
      address,
      name: l.args.name,
      metadataURI: l.args.metadataURI,
      organizer: l.args.organizer,
      arbiter: await ev.arbiter(),
      attesters: [...(await ev.attesters())],
      threshold: Number(await ev.threshold()),
      challengeWindow: Number(await ev.challengeWindow()),
      cancelled: await ev.cancelled(),
      createdAt: await timeOf(l.blockNumber),
      tokens: [...accepted],
      totals: {},
      donations: [],
      milestones: [],
    };
    for (const t of accepted) {
      e.totals[t] = {
        donated: units(t, await ev.totalDonated(t)),
        committed: units(t, await ev.committed(t)),
        released: units(t, await ev.released(t)),
      };
    }

    const n = Number(await ev.milestoneCount());
    for (let i = 0; i < n; i++) {
      const m = await ev.milestone(i);
      e.milestones.push({
        id: i,
        descriptionURI: m.descriptionURI,
        payee: m.payee,
        token: m.token,
        amount: units(m.token, m.amount),
        status: STATUS[Number(m.status)],
        approvals: Number(m.approvals),
        challengeDeadline: Number(m.challengeDeadline),
        attestations: [],
        revocations: [],
        challenges: [],
        rulings: [],
      });
    }

    for (const log of await ev.queryFilter("*")) {
      const name = log.fragment?.name;
      const a = log.args?.toObject() ?? {};
      const s = await stamp(log);
      const ms = a.id !== undefined ? e.milestones[Number(a.id)] : null;
      if (name === "Donated") e.donations.push({ donor: a.donor, token: a.token, amount: units(a.token, a.amount), ...s });
      else if (name === "MilestoneProposed") Object.assign(ms, { proposedAt: s.time, proposedTx: s.tx });
      else if (name === "Attested")
        ms.attestations.push({ attester: a.attester, evidenceHash: a.evidenceHash, evidenceURI: a.evidenceURI, ...s });
      else if (name === "AttestationRevoked") ms.revocations.push({ attester: a.attester, reasonURI: a.reasonURI, ...s });
      else if (name === "MilestoneApproved") ms.approvedAt = s.time;
      else if (name === "Challenged") ms.challenges.push({ by: a.by, reasonURI: a.reasonURI, ...s });
      else if (name === "DisputeResolved") ms.rulings.push({ upheld: a.upheld, reasonURI: a.reasonURI, ...s });
      else if (name === "Released") Object.assign(ms, { releasedAt: s.time, releasedTx: s.tx });
      else if (name === "Cancelled") Object.assign(e, { cancelledAt: s.time, cancelReasonURI: a.reasonURI });
    }
    events.push(e);
  }

  // ---- deeds
  const deeds = {};
  for (const log of await impact.queryFilter("*")) {
    const name = log.fragment?.name;
    const a = log.args?.toObject() ?? {};
    const s = await stamp(log);
    if (name === "DeedReported") {
      deeds[a.deedId] = {
        id: a.deedId,
        event: a.eventAddr,
        milestoneId: Number(a.milestoneId),
        reporter: a.reporter,
        category: a.category,
        lat: Number(a.latE5) / 1e5,
        lon: Number(a.lonE5) / 1e5,
        reportURI: a.reportURI,
        reportHash: a.reportHash,
        photos: [],
        votes: [],
        ...s,
      };
    } else if (name === "PhotoRegistered") deeds[a.deedId].photos.push({ hash: a.photoHash, uri: a.uri });
    else if (name === "DeedVoted")
      deeds[a.deedId].votes.push({ voter: a.voter, confirms: a.confirms, verified: a.verifiedVoter, noteURI: a.noteURI, ...s });
  }

  const net = await provider.getNetwork();
  const head = await provider.getBlock("latest");
  return {
    meta: {
      ...meta,
      chainId: Number(net.chainId),
      block: head.number,
      asOf: head.timestamp,
      contracts: {
        identity: await identity.getAddress(),
        registry: await registry.getAddress(),
        impact: await impact.getAddress(),
      },
    },
    issuers,
    orgs,
    labels,
    tokens,
    events,
    deeds: Object.values(deeds),
    docs,
    blocked,
  };
}

module.exports = { buildSnapshot };
