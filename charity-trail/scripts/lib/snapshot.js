// Reads every log from the registries, events, vaults and forum, and builds
// one JSON snapshot for the tracker site. Nothing here is trusted state:
// it's all rebuilt from public chain data (plus the off-chain documents,
// which the site re-hashes against the on-chain hashes).
const { ethers } = require("hardhat");

const STATUS = ["Proposed", "Released", "Withdrawn"];

async function buildSnapshot({ identity, registry, impact, vaults, forum, labels = {}, docs = {}, blocked = [], meta = {} }) {
  const provider = ethers.provider;
  const blockTimes = {};
  const timeOf = async (bn) => {
    if (!(bn in blockTimes)) blockTimes[bn] = (await provider.getBlock(bn)).timestamp;
    return blockTimes[bn];
  };
  // block + index give a total order, which money tracing needs.
  const stamp = async (log) => ({ time: await timeOf(log.blockNumber), block: log.blockNumber, index: log.index, tx: log.transactionHash });
  const argsOf = (log) => log.args?.toObject() ?? {};

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
    const accepted = await ev.acceptedTokens();
    for (const t of accepted) await tokenInfo(t);

    const e = {
      address: l.args.eventAddress,
      name: l.args.name,
      metadataURI: l.args.metadataURI,
      organizer: l.args.organizer,
      attesters: [...(await ev.attesters())],
      threshold: Number(await ev.threshold()),
      closed: await ev.closed(),
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
        // Forum subject id; equals the deed id if this milestone gets one.
        subject: ethers.keccak256(ethers.AbiCoder.defaultAbiCoder().encode(["address", "uint256"], [l.args.eventAddress, i])),
        attestations: [],
      });
    }
    for (const log of await ev.queryFilter("*")) {
      const name = log.fragment?.name;
      const a = argsOf(log);
      const s = await stamp(log);
      const ms = a.id !== undefined ? e.milestones[Number(a.id)] : null;
      if (name === "Donated")
        e.donations.push({ donor: a.donor, token: a.token, amount: units(a.token, a.amount), via: a.via, ...s });
      else if (name === "MilestoneProposed") Object.assign(ms, { proposedAt: s.time });
      else if (name === "Attested")
        ms.attestations.push({ attester: a.attester, evidenceHash: a.evidenceHash, evidenceURI: a.evidenceURI, ...s });
      else if (name === "MilestoneWithdrawn") Object.assign(ms, { withdrawnAt: s.time, withdrawReasonURI: a.reasonURI });
      else if (name === "Released") Object.assign(ms, { releasedAt: s.time, releasedBlock: s.block, releasedIndex: s.index, releasedTx: s.tx });
      else if (name === "Closed") Object.assign(e, { closedAt: s.time, closeReasonURI: a.reasonURI });
    }
    events.push(e);
  }

  // ---- deeds
  const deeds = {};
  for (const log of await impact.queryFilter("*")) {
    const name = log.fragment?.name;
    const a = argsOf(log);
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
        sensitive: a.sensitive,
        reportURI: a.reportURI,
        reportHash: a.reportHash,
        photos: [],
        contested: [],
        ...s,
      };
    } else if (name === "PhotoRegistered") deeds[a.deedId].photos.push({ hash: a.photoHash, uri: a.uri });
    else if (name === "DeedContested") {
      deeds[a.deedId].contested.push({ with: a.otherDeedId, reason: a.reason, ...s });
      deeds[a.otherDeedId].contested.push({ with: a.deedId, reason: a.reason, ...s });
    }
  }

  // ---- vaults (recurring giving, redirects, curators)
  const vaultLog = [];
  for (const log of await vaults.queryFilter("*")) {
    const name = log.fragment?.name;
    const a = argsOf(log);
    const s = await stamp(log);
    const row = { type: name, ...s };
    if (a.donor) row.who = a.donor;
    if (a.who) row.who = a.who;
    if (a.token) {
      await tokenInfo(a.token);
      row.token = a.token;
    }
    if (a.amount !== undefined) row.amount = units(a.token, a.amount);
    if (a.monthly !== undefined) row.monthly = units(a.token, a.monthly);
    if (a.target) row.target = a.target;
    if (a.curator !== undefined) row.curator = a.curator;
    if (a.targets) Object.assign(row, { targets: [...a.targets], bps: [...a.bps].map(Number) });
    vaultLog.push(row);
  }

  // ---- forum
  const posts = [];
  const delegations = [];
  for (const log of await forum.queryFilter("*")) {
    const name = log.fragment?.name;
    const a = argsOf(log);
    const s = await stamp(log);
    if (name === "Posted")
      posts.push({ id: a.id, author: a.author, subject: a.subject, replyTo: a.replyTo, stance: Number(a.stance), uri: a.uri, hash: a.contentHash, ...s });
    else if (name === "Delegated") delegations.push({ from: a.from, to: a.to, ...s });
  }

  const net = await provider.getNetwork();
  const head = await provider.getBlock("latest");
  return {
    meta: {
      ...meta,
      chainId: Number(net.chainId),
      block: head.number,
      asOf: head.timestamp,
      contracts: Object.fromEntries(
        await Promise.all(Object.entries({ identity, registry, impact, vaults, forum }).map(async ([k, c]) => [k, await c.getAddress()]))
      ),
    },
    issuers,
    orgs,
    labels,
    tokens,
    events,
    deeds: Object.values(deeds),
    vaults: vaultLog,
    posts,
    delegations,
    docs,
    blocked,
  };
}

module.exports = { buildSnapshot };
