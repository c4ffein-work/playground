// Follows one donor's money through events, in time order.
//
// Each event's pool is replayed chronologically. When a payment goes out, it
// is drawn from every donor's *unspent* balance in that pool at that moment,
// pro rata. So a gift made in October never gets credit for a payment made
// in April. A grant into a child event is a donor there like any other; your
// part of it is followed into the child the same way.
//
// Weights for the public discussion are derived from the same data: what an
// address gave that actually left escrow through signed-off milestones.
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.Flow = factory();
})(typeof self !== "undefined" ? self : this, function () {
  const EPS = 1e-9;

  function index(data) {
    if (data.__ix) return data.__ix;
    const events = Object.fromEntries(data.events.map((e) => [e.address, e]));
    const deeds = {};
    for (const d of data.deeds) deeds[`${d.event}:${d.milestoneId}`] = d;
    return (data.__ix = { events, deeds, ledgers: {} });
  }

  /// Replays one (event, token) pool. Returns, per donor key:
  /// inflow, the part of each released milestone, of each open one, and what's left.
  function ledger(ix, ev, token) {
    const key = `${ev.address}|${token}`;
    if (ix.ledgers[key]) return ix.ledgers[key];
    const items = [];
    for (const d of ev.donations) if (d.token === token) items.push({ b: d.block, i: d.index, d });
    for (const m of ev.milestones)
      if (m.token === token && m.status === "Released") items.push({ b: m.releasedBlock, i: m.releasedIndex, m });
    items.sort((x, y) => x.b - y.b || x.i - y.i);

    const remaining = {};
    const inflow = {};
    const paid = {};
    let total = 0;
    const draw = (amount) => {
      const out = {};
      if (total <= EPS) return out;
      const f = Math.min(1, amount / total);
      for (const k of Object.keys(remaining)) {
        const take = remaining[k] * f;
        if (take > EPS) out[k] = take;
        remaining[k] -= take;
      }
      total -= Math.min(amount, total);
      return out;
    };
    for (const it of items) {
      if (it.d) {
        remaining[it.d.donor] = (remaining[it.d.donor] || 0) + it.d.amount;
        inflow[it.d.donor] = (inflow[it.d.donor] || 0) + it.d.amount;
        total += it.d.amount;
      } else paid[it.m.id] = draw(it.m.amount);
    }
    const pending = {};
    for (const m of ev.milestones) if (m.token === token && m.status === "Proposed") pending[m.id] = draw(m.amount);
    return (ix.ledgers[key] = { inflow, paid, pending, unspent: { ...remaining } });
  }

  /// Node kinds: event, payment, pending, unspent, refundable.
  function expand(ix, ev, token, who, mult, depth) {
    const L = ledger(ix, ev, token);
    const amount = (L.inflow[who] || 0) * mult;
    const pool = ev.totals[token].donated;
    const node = { kind: "event", event: ev, token, amount, share: pool ? amount / pool : 0, children: [] };
    if (depth > 8) return node;

    const intoChild = {};
    for (const m of ev.milestones) {
      if (m.token !== token) continue;
      if (m.status === "Released") {
        const part = (L.paid[m.id]?.[who] || 0) * mult;
        if (part <= EPS) continue;
        if (ix.events[m.payee]) {
          const c = (intoChild[m.payee] = intoChild[m.payee] || { part: 0, milestones: [] });
          c.part += part;
          c.milestones.push(m);
        } else {
          node.children.push({ kind: "payment", event: ev, milestone: m, token, amount: part, deed: ix.deeds[`${ev.address}:${m.id}`] || null });
        }
      } else if (m.status === "Proposed") {
        const part = (L.pending[m.id]?.[who] || 0) * mult;
        if (part > EPS) node.children.push({ kind: "pending", event: ev, milestone: m, token, amount: part });
      }
    }
    for (const [addr, c] of Object.entries(intoChild)) {
      const child = ix.events[addr];
      const from = ledger(ix, child, token).inflow[ev.address] || c.part;
      const sub = expand(ix, child, token, ev.address, c.part / from, depth + 1);
      sub.milestone = c.milestones[0];
      sub.milestones = c.milestones;
      sub.parent = ev;
      node.children.push(sub);
    }
    const left = (L.unspent[who] || 0) * mult;
    if (left > 1e-6) node.children.push({ kind: ev.closed ? "refundable" : "unspent", event: ev, token, amount: left });
    return node;
  }

  /// One tree per (event, token) the donor gave to directly.
  function trace(data, donor) {
    const ix = index(data);
    const d = donor.toLowerCase();
    const roots = [];
    for (const ev of data.events) {
      for (const token of ev.tokens) {
        const mine = ev.donations.filter((x) => x.token === token && x.donor.toLowerCase() === d);
        if (!mine.length) continue;
        const tree = expand(ix, ev, token, mine[0].donor, 1, 0);
        tree.donations = mine;
        roots.push(tree);
      }
    }
    return roots;
  }

  function leaves(node, out = []) {
    if (node.kind !== "event") out.push(node);
    else for (const c of node.children) leaves(c, out);
    return out;
  }

  /// People and funds worth offering in a picker (not escrow contracts).
  function donors(data) {
    const ix = index(data);
    const sums = {};
    for (const ev of data.events)
      for (const d of ev.donations) {
        if (ix.events[d.donor]) continue;
        sums[d.donor] = (sums[d.donor] || 0) + d.amount;
      }
    return Object.entries(sums)
      .map(([address, total]) => ({ address, total }))
      .sort((a, b) => b.total - a.total);
  }

  /// What each address gave that has left escrow through signed milestones
  /// (any token, counted 1:1). This is the raw weight of a voice.
  function givingWeights(data) {
    if (data.__weights) return data.__weights;
    const w = {};
    for (const { address } of donors(data)) {
      w[address] = trace(data, address)
        .flatMap((t) => leaves(t))
        .filter((l) => l.kind === "payment")
        .reduce((s, l) => s + l.amount, 0);
    }
    return (data.__weights = w);
  }

  /// Latest delegation of each address (to = 0x0 means taken back).
  function delegations(data) {
    const out = {};
    for (const d of data.delegations) out[d.from] = /^0x0+$/.test(d.to) ? null : d.to;
    return out;
  }

  /// Summarise a discussion. Each author's latest stance counts. Weight =
  /// own giving + giving of people who delegated to them and didn't speak
  /// on this subject themselves.
  function discussion(data, subject) {
    const posts = data.posts.filter((p) => p.subject.toLowerCase() === subject.toLowerCase()).sort((a, b) => a.block - b.block || a.index - b.index);
    const weights = givingWeights(data);
    const dele = delegations(data);
    const latest = {};
    for (const p of posts) latest[p.author] = p;
    const spoke = new Set(Object.keys(latest));
    const lent = {};
    for (const [from, to] of Object.entries(dele)) {
      if (!to || spoke.has(from) || !spoke.has(to)) continue;
      (lent[to] = lent[to] || []).push(from);
    }
    const voices = Object.values(latest).map((p) => {
      const own = weights[p.author] || 0;
      const fromOthers = (lent[p.author] || []).reduce((s, a) => s + (weights[a] || 0), 0);
      return { post: p, author: p.author, stance: p.stance, own, delegated: fromOthers, delegators: lent[p.author] || [], weight: own + fromOthers };
    });
    const sum = (f) => voices.filter(f).reduce((s, v) => s + v.weight, 0);
    return {
      posts,
      voices,
      weighted: { support: sum((v) => v.stance > 0), dispute: sum((v) => v.stance < 0) },
    };
  }

  /// Monthly totals given by a donor, per destination event.
  function monthly(data, donor) {
    const d = donor.toLowerCase();
    const rows = {};
    for (const ev of data.events)
      for (const x of ev.donations) {
        if (x.donor.toLowerCase() !== d) continue;
        const m = new Date(x.time * 1000).toISOString().slice(0, 7);
        rows[m] = rows[m] || {};
        rows[m][ev.address] = (rows[m][ev.address] || 0) + x.amount;
      }
    return rows;
  }

  return { trace, leaves, donors, index, ledger, givingWeights, delegations, discussion, monthly };
});
