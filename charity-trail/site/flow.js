// Follows one donor's money through events, pro rata.
// Pooled money is fungible, so "your share" of every payout is
// (what you gave / everything the pool received) × payout. Each hop into a
// child event repeats the same rule with the child's own pool.
(function (root, factory) {
  if (typeof module === "object" && module.exports) module.exports = factory();
  else root.Flow = factory();
})(typeof self !== "undefined" ? self : this, function () {
  function index(data) {
    const events = Object.fromEntries(data.events.map((e) => [e.address, e]));
    const deeds = {};
    for (const d of data.deeds) deeds[`${d.event}:${d.milestoneId}`] = d;
    return { events, deeds };
  }

  /// Node kinds: event, payment, pending, unspent, refundable.
  function expand(ix, eventAddr, token, amount, depth) {
    const ev = ix.events[eventAddr];
    const pool = ev.totals[token].donated;
    const share = pool > 0 ? amount / pool : 0;
    const node = { kind: "event", event: ev, token, amount, share, children: [] };
    if (depth > 8) return node; // cycles are impossible on-chain, but be safe

    for (const m of ev.milestones) {
      if (m.token !== token || m.status === "Rejected") continue;
      const part = share * m.amount;
      if (m.status === "Released") {
        if (ix.events[m.payee]) {
          const child = expand(ix, m.payee, token, part, depth + 1);
          child.milestone = m;
          child.parent = ev;
          node.children.push(child);
        } else {
          node.children.push({
            kind: "payment",
            event: ev,
            milestone: m,
            token,
            amount: part,
            deed: ix.deeds[`${ev.address}:${m.id}`] || null,
          });
        }
      } else {
        node.children.push({ kind: "pending", event: ev, milestone: m, token, amount: part });
      }
    }
    const t = ev.totals[token];
    const free = share * (t.donated - t.committed);
    if (free > 1e-9) node.children.push({ kind: ev.cancelled ? "refundable" : "unspent", event: ev, token, amount: free });
    return node;
  }

  /// One tree per (event, token) the donor gave to.
  function trace(data, donor) {
    const ix = index(data);
    const given = {};
    const firstAt = {};
    for (const ev of data.events) {
      for (const d of ev.donations) {
        if (d.donor.toLowerCase() !== donor.toLowerCase()) continue;
        const k = `${ev.address}|${d.token}`;
        given[k] = (given[k] || 0) + d.amount;
        firstAt[k] = Math.min(firstAt[k] || Infinity, d.time);
      }
    }
    return Object.entries(given).map(([k, amount]) => {
      const [eventAddr, token] = k.split("|");
      const tree = expand(ix, eventAddr, token, amount, 0);
      tree.donatedAt = firstAt[k];
      return tree;
    });
  }

  function leaves(node, out = []) {
    if (!node.children || node.children.length === 0 || node.kind !== "event") {
      if (node.kind !== "event") out.push(node);
      return out;
    }
    for (const c of node.children) leaves(c, out);
    return out;
  }

  /// Donors worth offering in a picker: people, not escrow contracts.
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

  return { trace, leaves, donors, index };
});
