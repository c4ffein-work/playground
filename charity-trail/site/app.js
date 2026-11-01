(() => {
  const T = window.TRAIL;
  const F = window.Flow;
  const IX = F.index(T);
  const app = document.getElementById("app");
  const dlg = document.getElementById("dlg");
  const dlgBody = document.getElementById("dlg-body");

  // ------------------------------------------------------------ helpers
  const esc = (s) =>
    String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  const short = (a) => (a ? `${a.slice(0, 6)}…${a.slice(-4)}` : "");
  const same = (a, b) => a && b && a.toLowerCase() === b.toLowerCase();
  const sym = (t) => T.tokens[t]?.symbol || "";
  const nf2 = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const nf0 = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });
  const num = (v) => (Math.abs(v - Math.round(v)) < 0.005 && Math.abs(v) >= 100 ? nf0.format(v) : nf2.format(v));
  const money = (v, t) => `${num(v)} ${sym(t)}`;
  const df = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" });
  const mf = new Intl.DateTimeFormat("en-GB", { month: "short", timeZone: "UTC" });
  const date = (ts) => (ts ? df.format(ts * 1000) : "");
  const name = (a) => T.orgs[a]?.name || T.labels[a] || IX.events[a]?.name || short(a);
  const isYou = (a) => same(a, T.meta.you);
  const verified = (a) => !!T.orgs[a]?.verified;
  const who = (a) => {
    const o = T.orgs[a];
    const tag = o ? (o.verified ? ` <span class="badge" title="Identity verified by ${esc(T.issuers[o.issuer]?.name)}">✓ verified</span>` : ` <span class="badge no">revoked</span>`) : "";
    return `<span class="who">${esc(isYou(a) ? "You" : name(a))}</span>${tag}`;
  };
  const doc = (uri) => T.docs[uri];
  const docTitle = (uri, fallback = "") => doc(uri)?.title || fallback;
  const cap = (x) => x.charAt(0).toUpperCase() + x.slice(1);
  const budgetTitle = (m) => cap(docTitle(m.descriptionURI, `Milestone #${m.id}`).replace(/^Budget line: /, ""));
  const svgSrc = (svg) => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  const eventNo = (addr) => T.events.findIndex((e) => e.address === addr);
  const eventColor = (addr) => `var(--c${(eventNo(addr) % 3) + 1})`;
  const deedNo = (d) => T.deeds.indexOf(d);
  const deedPlace = (d) => docTitle(d.reportURI).split(", ").pop() || "";
  const categoryName = (c) => ({ "water-well": "Water well", "water-tank": "Water tank" })[c] || c;
  const deedName = (d) => `${categoryName(d.category)}, ${deedPlace(d)}`;
  const deedCover = (d) => doc(d.photos[d.photos.length - 1]?.uri)?.content;
  const addressSubject = (a) => "0x" + "0".repeat(24) + a.slice(2).toLowerCase();
  const monthKey = (ts) => new Date(ts * 1000).toISOString().slice(0, 7);
  const monthLabel = (k) => mf.format(Date.parse(`${k}-15T00:00:00Z`));
  const pct = (bps) => `${Math.round(bps / 100)}%`;

  async function sha256(text) {
    const buf = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
    return "0x" + [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
  }
  // Re-hash every displayed file and compare with the hash stored on-chain.
  async function runChecks(rootEl = document) {
    for (const el of rootEl.querySelectorAll("[data-check]")) {
      const d = doc(el.dataset.check);
      const want = (el.dataset.hash || "").toLowerCase();
      if (!d || !want) continue;
      try {
        const got = await sha256(d.content);
        el.className = `check ${got === want ? "ok" : "bad"}`;
        el.textContent = got === want ? "✓ File matches its on-chain hash" : "✗ File does not match its on-chain hash";
        el.title = `sha256 ${got}`;
      } catch {
        el.className = "check wait";
        el.textContent = "Hash check unavailable in this browser";
      }
    }
  }
  const check = (uri, hash) => `<span class="check wait" data-check="${esc(uri)}" data-hash="${esc(hash)}">Checking hash…</span>`;

  function openDialog(html) {
    dlgBody.innerHTML = `${html}<div class="row"><button class="btn" data-close type="button">Close</button></div>`;
    if (typeof dlg.showModal === "function") dlg.showModal();
    else dlg.setAttribute("open", "");
    runChecks(dlgBody);
    dlgBody.querySelector("[data-close]").focus();
  }
  function openDoc(uri, hash) {
    const d = doc(uri);
    if (!d) return openDialog(`<h2 id="dlg-title">Document not available</h2><p class="mono">${esc(uri)}</p>`);
    const body = d.type === "svg" ? `<img src="${svgSrc(d.content)}" alt="${esc(d.title)}">` : `<pre class="doc">${esc(d.content)}</pre>`;
    openDialog(
      `<h2 id="dlg-title">${esc(d.title)}</h2>${body}
       <dl class="kv"><dt>Location</dt><dd class="mono">${esc(uri)}</dd>
       ${hash ? `<dt>On-chain hash</dt><dd class="mono">${esc(hash)}</dd><dt>Check</dt><dd>${check(uri, hash)}</dd>` : ""}</dl>`
    );
  }

  const state = { donor: T.meta.you };
  document.addEventListener("click", async (e) => {
    const t = e.target.closest("[data-doc],[data-close],[data-copy],[data-photo],[data-donor]");
    if (!t) return;
    if (t.hasAttribute("data-close")) return dlg.close();
    if (t.dataset.doc) return openDoc(t.dataset.doc, t.dataset.hash);
    if (t.dataset.photo) return openDoc(t.dataset.photo, t.dataset.hash);
    if (t.dataset.donor) {
      state.donor = t.dataset.donor;
      if (location.hash === "#trace") render();
      else location.hash = "trace";
      return;
    }
    if (t.dataset.copy) {
      try {
        await navigator.clipboard.writeText(t.dataset.copy);
        t.textContent = "Copied";
      } catch {
        t.textContent = "Copy blocked: select the text instead";
      }
    }
  });
  dlg.addEventListener("click", (e) => {
    if (e.target === dlg) dlg.close();
  });

  const demoNote = `<p class="demo-note">Demo data: fictional organizations on a local test chain, as of ${date(T.meta.asOf)}. Photos are illustrations; their hashes are real and checked live.</p>`;

  // ------------------------------------------------------------ vaults
  const vaultRows = (a) => T.vaults.filter((r) => same(r.who, a));
  const allocAt = (a, time) => [...T.vaults].reverse().find((r) => r.type === "AllocationSet" && same(r.who, a) && r.time <= time);
  const followingAt = (a, time) => {
    const r = [...T.vaults].reverse().find((x) => same(x.who, a) && x.time <= time && (x.type === "Followed" || x.type === "AllocationSet"));
    return r?.type === "Followed" && !/^0x0+$/.test(r.curator) ? r.curator : null;
  };
  const splitAt = (a, time) => {
    const c = followingAt(a, time);
    return { curator: c, alloc: allocAt(c || a, time) };
  };
  const splitHTML = (alloc) =>
    alloc
      ? `<div class="split">${alloc.targets
          .map(
            (t, i) => `<div class="split-row"><a href="#event-${eventNo(t)}">${esc(name(t))}</a><b class="num">${pct(alloc.bps[i])}</b>
            <div class="split-bar"><i style="--c:${eventColor(t)};width:${alloc.bps[i] / 100}%"></i></div></div>`
          )
          .join("")}</div>`
      : `<p class="muted">No split set.</p>`;
  const curators = () => {
    const set = new Set();
    for (const r of T.vaults) if (r.type === "Followed" && !/^0x0+$/.test(r.curator)) set.add(r.curator);
    return [...set];
  };

  // ------------------------------------------------------------- charts
  function monthBars(rows, keys, colorOf, { height = 170, unit = "" } = {}) {
    // rows: { "2026-03": { key: value } }
    const months = Object.keys(rows).sort();
    if (!months.length) return "";
    const max = Math.max(...months.map((m) => Object.values(rows[m]).reduce((s, v) => s + v, 0)));
    const W = 600, H = height, pad = 22, top = 18;
    const bw = (W - pad * 2) / months.length;
    let g = `<line x1="${pad}" x2="${W - pad}" y1="${H - pad}" y2="${H - pad}" stroke="var(--line)"/>`;
    months.forEach((m, i) => {
      let y = H - pad;
      let tot = 0;
      for (const k of keys) {
        const v = rows[m][k] || 0;
        if (!v) continue;
        const h = (v / max) * (H - pad - top);
        y -= h;
        tot += v;
        g += `<rect x="${pad + i * bw + bw * 0.18}" y="${y}" width="${bw * 0.64}" height="${Math.max(h, 1)}" fill="${colorOf(k)}"><title>${esc(name(k))}: ${num(v)}${unit}</title></rect>`;
      }
      g += `<text class="v" x="${pad + i * bw + bw / 2}" y="${y - 5}" text-anchor="middle">${num(tot)}</text>`;
      g += `<text x="${pad + i * bw + bw / 2}" y="${H - 6}" text-anchor="middle">${monthLabel(m)}</text>`;
    });
    return `<div class="chart"><svg viewBox="0 0 ${W} ${H}" role="img" aria-label="Amounts per month">${g}</svg></div>`;
  }
  const legend = (keys, colorOf) => `<div class="legend">${keys.map((k) => `<span><i style="--c:${colorOf(k)}"></i>${esc(name(k))}</span>`).join("")}</div>`;

  // -------------------------------------------------------------- trace
  const KIND = {
    deed: { label: "Built and publicly reported", c: "var(--ok)" },
    payment: { label: "Paid for goods and services", c: "var(--accent)" },
    pending: { label: "Waiting for signatures", c: "var(--warn)" },
    unspent: { label: "Not spent yet, in escrow", c: "var(--muted)" },
    refundable: { label: "Refundable (event closed)", c: "var(--bad)" },
  };
  const kindOf = (leaf) => (leaf.kind === "payment" ? (leaf.deed ? "deed" : "payment") : leaf.kind);

  function givingCard(donor, donorName) {
    const rows = vaultRows(donor);
    const monthlySet = rows.filter((r) => r.type === "MonthlySet");
    const byMonth = F.monthly(T, donor);
    const dests = [...new Set(Object.values(byMonth).flatMap((r) => Object.keys(r)))];
    const chart = `<h3>Given per month</h3>${monthBars(byMonth, dests, eventColor)}${legend(dests, eventColor)}`;
    if (!monthlySet.length) return `<section class="card">${chart}<p class="small muted">One-off gifts only. No monthly plan.</p></section>`;

    const now = T.meta.asOf;
    const { curator, alloc } = splitAt(donor, now);
    const hist = [];
    for (const r of rows) {
      if (r.type === "MonthlySet") hist.push({ t: r.time, h: `Monthly gift set to ${money(r.monthly, r.token)}` });
      if (r.type === "AllocationSet") hist.push({ t: r.time, h: `Own split: ${r.targets.map((x, i) => `${pct(r.bps[i])} ${esc(name(x))}`).join(", ")}` });
      if (r.type === "Followed" && !/^0x0+$/.test(r.curator)) hist.push({ t: r.time, h: `Started following <a href="#curator-${curators().indexOf(r.curator)}">${esc(name(r.curator))}</a>` });
    }
    // Curator changes that applied while following.
    for (const r of T.vaults)
      if (r.type === "AllocationSet" && same(followingAt(donor, r.time), r.who) && !rows.includes(r))
        hist.push({ t: r.time, h: `${esc(name(r.who))} changed its split: ${r.targets.map((x, i) => `${pct(r.bps[i])} ${esc(name(x))}`).join(", ")}` });
    hist.sort((a, b) => a.t - b.t);
    const ms = monthlySet[monthlySet.length - 1];
    const call = curator ? `vaults.follow(${short(curator)})` : `vaults.setAllocation([${alloc?.targets.map(short).join(", ")}], [${alloc?.bps.join(", ")}])`;

    return `<section class="card"><div class="row between"><h2>${isYou(donor) ? "Your" : esc(donorName) + "'s"} monthly gift</h2><span class="pill info">${money(ms.monthly, ms.token)} a month</span></div>
      <div class="grid2"><div class="stack"><h3>Where it goes now</h3>
        ${curator ? `<p class="small">Following <a href="#curator-${curators().indexOf(curator)}">${esc(name(curator))}</a>. Their split applies from the next payment.</p>` : `<p class="small">${isYou(donor) ? "Your" : "Their"} own split.</p>`}
        ${splitHTML(alloc)}</div>
        <div class="stack"><h3>Changes</h3><div class="history">${hist.map((x) => `<div><span class="small muted">${date(x.t)}</span><br>${x.h}</div>`).join("")}</div></div></div>
      ${chart}
      <details><summary>How to redirect or stop</summary><div class="stack" style="margin-top:8px">
        <p class="small">One transaction from ${isYou(donor) ? "your" : "the donor's"} wallet changes where the next payment goes. Money already given stays with the event it went to, so organizations never hold more than a month of it in advance. Unspent balance can be withdrawn any time.</p>
        <pre class="code">${esc(call)}          // current setting
vaults.setAllocation([eventA, eventB], [6000, 4000])   // your own split
vaults.follow(curator)                                // copy a curator
vaults.withdraw(token, amount)                        // take back what's not given yet</pre>
        <p class="small muted">GivingVaults ${esc(T.meta.contracts.vaults)}</p></div></details></section>`;
  }

  function viewTrace() {
    const trees = F.trace(T, state.donor);
    const options = F.donors(T)
      .map((d) => `<option value="${d.address}"${d.address === state.donor ? " selected" : ""}>${esc(isYou(d.address) ? "You" : name(d.address))}</option>`)
      .join("");
    const donorName = isYou(state.donor) ? "You" : name(state.donor);
    let html = `${demoNote}<section class="hero">
      <div class="picker"><label for="donor">Follow the money of</label><select id="donor">${options}</select></div>`;
    if (!trees.length) return `${html}<p>No donations found for this wallet.</p></section>`;

    const byToken = {};
    for (const t of trees) byToken[t.token] = (byToken[t.token] || 0) + t.amount;
    const first = Math.min(...trees.flatMap((t) => t.donations.map((d) => d.time)));
    const gifts = trees.reduce((s, t) => s + t.donations.length, 0);
    html += `<h1 class="lead">${esc(donorName)} gave <strong>${Object.entries(byToken).map(([t, v]) => money(v, t)).join(" and ")}</strong> in ${gifts} gift${gifts > 1 ? "s" : ""} since ${date(first)}. Here is where it went.</h1>`;

    for (const token of Object.keys(byToken)) {
      const sums = {};
      for (const tree of trees.filter((t) => t.token === token)) for (const l of F.leaves(tree)) sums[kindOf(l)] = (sums[kindOf(l)] || 0) + l.amount;
      const keys = Object.keys(KIND).filter((k) => sums[k] > 0.004);
      html += `<div class="tiles">${keys.map((k) => `<div class="tile" style="--c:${KIND[k].c}"><b class="num">${money(sums[k], token)}</b><span>${KIND[k].label}</span></div>`).join("")}</div>
        <div class="bar" role="img" aria-label="Split of ${money(byToken[token], token)}">${keys.map((k) => `<i style="--c:${KIND[k].c};flex:${sums[k]}" title="${KIND[k].label}: ${money(sums[k], token)}"></i>`).join("")}</div>`;
    }
    html += `</section>${givingCard(state.donor, donorName)}`;
    html += `<section class="card"><div class="row between"><h2>The path, org by org</h2><span class="scroll-hint">Scroll sideways on small screens</span></div>
        <div class="flow-wrap">${sankey(trees, donorName)}</div>
        <p class="small muted">Each pool is replayed in time order. A payment is drawn from every donor's unspent money in the pool at that moment, so later gifts never get credit for earlier payments.</p></section>
      <section class="card"><h2>Step by step</h2><div class="journey">${trees.map((t) => hop(t)).join("")}</div></section>`;
    return html;
  }

  function sankey(trees, donorName) {
    const nodes = [];
    const push = (n, depth, parent) => {
      const node = { ...n, depth, parent, kids: [] };
      nodes.push(node);
      if (parent) parent.kids.push(node);
      return node;
    };
    const total = trees.reduce((s, t) => s + t.amount, 0);
    const tokens = [...new Set(trees.map((t) => t.token))];
    const root = push({ label: donorName, sub: tokens.map((tk) => money(trees.filter((t) => t.token === tk).reduce((s, t) => s + t.amount, 0), tk)).join(" + "), value: total, c: "var(--accent)" }, 0, null);
    const walk = (n, depth, parent) => {
      if (n.kind === "event") {
        const me = push({ label: n.event.name, sub: money(n.amount, n.token), value: n.amount, c: eventColor(n.event.address), href: `#event-${eventNo(n.event.address)}` }, depth, parent);
        n.children.forEach((c) => walk(c, depth + 1, me));
      } else {
        const k = kindOf(n);
        const label = k === "unspent" || k === "refundable" ? "Still in escrow" : n.deed ? deedName(n.deed) : budgetTitle(n.milestone);
        const sub = k === "pending" ? `${money(n.amount, n.token)} · waiting` : money(n.amount, n.token);
        push({ label, sub, value: n.amount, c: KIND[k].c, href: n.deed ? `#deed-${deedNo(n.deed)}` : null }, depth, parent);
      }
    };
    trees.forEach((t) => walk(t, 1, root));

    const cols = Math.max(...nodes.map((n) => n.depth)) + 1;
    const colW = 200, nodeW = 10, gap = 10, slot = 34, padY = 10;
    const scale = 160 / total;
    const byCol = Array.from({ length: cols }, (_, d) => nodes.filter((n) => n.depth === d));
    const colH = byCol.map((c) => c.reduce((s, n) => s + Math.max(n.value * scale, 2, slot) + gap, -gap));
    const H = Math.max(...colH) + padY * 2;
    byCol.forEach((col, d) => {
      let y = padY + (H - padY * 2 - colH[d]) / 2;
      for (const n of col) {
        n.h = Math.max(n.value * scale, 2);
        n.x = 8 + d * colW;
        n.y = y;
        y += Math.max(n.h, slot) + gap;
      }
    });
    const W = 8 + (cols - 1) * colW + nodeW + 190;
    let links = "";
    for (const p of nodes) {
      let off = 0;
      for (const k of p.kids) {
        const x0 = p.x + nodeW, y0 = p.y + off, x1 = k.x, y1 = k.y, h = k.h, xm = (x0 + x1) / 2;
        links += `<path d="M${x0} ${y0}C${xm} ${y0} ${xm} ${y1} ${x1} ${y1}L${x1} ${y1 + h}C${xm} ${y1 + h} ${xm} ${y0 + h} ${x0} ${y0 + h}Z" fill="${k.c}" fill-opacity="0.22"/>`;
        off += k.h;
      }
    }
    const trunc = (s, n = 28) => (s.length > n ? s.slice(0, n - 1) + "…" : s);
    const halo = `paint-order="stroke" stroke="var(--surface)" stroke-width="3" stroke-linejoin="round"`;
    const marks = nodes
      .map((n) => {
        const label = `<text x="${n.x + nodeW + 6}" y="${n.y + 12}" ${halo}>${esc(trunc(n.label))}</text><text class="sub" x="${n.x + nodeW + 6}" y="${n.y + 26}" ${halo}>${esc(n.sub)}</text>`;
        const rect = `<rect x="${n.x}" y="${n.y}" width="${nodeW}" height="${n.h}" rx="2" fill="${n.c}"/>`;
        return n.href ? `<a href="${n.href}">${rect}${label}</a>` : rect + label;
      })
      .join("");
    return `<svg width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" role="img" aria-label="Flow of ${esc(donorName)}'s donations">${links}${marks}</svg>`;
  }

  const sigChips = (m) =>
    m.attestations.map((a) => `<button type="button" class="chip" data-doc="${esc(a.evidenceURI)}" data-hash="${esc(a.evidenceHash)}">${esc(name(a.attester))} · ${date(a.time)}</button>`).join("");

  function deedMini(d) {
    const D = F.discussion(T, d.id);
    return `<a class="deed-mini" href="#deed-${deedNo(d)}">
      <img src="${svgSrc(deedCover(d))}" alt="Photo of ${esc(deedName(d))}">
      <span class="stack" style="gap:4px"><b>${esc(deedName(d))}</b>
      <span class="row">${deedPill(d, D)}</span>
      <span class="small muted">${d.photos.length} photo${d.photos.length > 1 ? "s" : ""}, report filed ${date(d.time)}</span></span></a>`;
  }

  function deedPill(d, D = F.discussion(T, d.id)) {
    const rOrg = T.orgs[d.reporter]?.orgId;
    const orgs = D.voices.filter((v) => verified(v.author) && !same(v.author, d.reporter) && !(rOrg && T.orgs[v.author]?.orgId === rOrg));
    const orgNo = orgs.filter((v) => v.stance < 0).length;
    const orgYes = orgs.filter((v) => v.stance > 0).length;
    const pills = [];
    if (d.contested.length) pills.push(`<span class="pill bad">Contested</span>`);
    if (d.sensitive) pills.push(`<span class="pill plain">Location protected</span>`);
    if (orgNo > orgYes) pills.push(`<span class="pill warn">Disputed by ${orgNo} verified org${orgNo > 1 ? "s" : ""}</span>`);
    else if (orgYes > 0) pills.push(`<span class="pill ok">Confirmed by ${orgYes} verified org${orgYes > 1 ? "s" : ""}</span>`);
    else if (!pills.length) pills.push(`<span class="pill info">Reported</span>`);
    return pills.join(" ");
  }

  function hop(n) {
    const dot = (c) => `<div class="rail" style="--c:${c}"><span class="dot"></span><span class="line"></span></div>`;
    if (n.kind === "event") {
      const ev = n.event;
      const via = n.milestone
        ? `<p class="small muted">Granted by ${esc(n.parent.name)} as “${esc(budgetTitle(n.milestone))}”, paid ${date(n.milestone.releasedAt)}.</p>
           <div class="sig">Signed off by ${sigChips(n.milestone)}</div>`
        : `<p class="small muted">${n.donations?.length || 0} gift${n.donations?.length === 1 ? "" : "s"}, ${date(n.donations?.[0]?.time)} to ${date(n.donations?.[n.donations.length - 1]?.time)}.</p>`;
      return `<div class="hop">${dot(eventColor(ev.address))}<div class="hop-body">
        <div class="hop-title"><h3><a href="#event-${eventNo(ev.address)}">${esc(ev.name)}</a></h3><span class="amount num">${money(n.amount, n.token)}</span></div>
        <p class="small">Escrow run by ${who(ev.organizer)}.</p>${via}
        <div class="indent journey">${n.children.map((c) => hop(c)).join("")}</div></div></div>`;
    }
    const k = kindOf(n);
    if (k === "unspent" || k === "refundable") {
      return `<div class="hop">${dot(KIND[k].c)}<div class="hop-body"><div class="hop-title"><h3>${KIND[k].label}</h3><span class="amount num">${money(n.amount, n.token)}</span></div>
        <p class="small muted">Held by the ${esc(n.event.name)} contract. It can only leave through a signed milestone${n.event.closed ? ", or as a refund" : ""}.</p></div></div>`;
    }
    const m = n.milestone;
    const head = `<div class="hop-title"><h3>${esc(budgetTitle(m))}</h3><span class="amount num">${money(n.amount, n.token)}</span></div>`;
    if (k === "pending") {
      return `<div class="hop">${dot(KIND.pending.c)}<div class="hop-body">${head}
        <div class="row"><span class="pill warn">Waiting for signatures</span><span class="small muted">${m.approvals} of ${n.event.threshold}, to ${esc(name(m.payee))}</span></div>
        <div class="sig">${sigChips(m)}</div></div></div>`;
    }
    return `<div class="hop">${dot(KIND[k].c)}<div class="hop-body">${head}
      <p class="small">Paid to ${who(m.payee)} on ${date(m.releasedAt)}.</p>
      <div class="sig">Signed off by ${sigChips(m)}</div>
      ${n.deed ? deedMini(n.deed) : ""}</div></div>`;
  }

  // --------------------------------------------------------- discussion
  /// `party`: the address whose claim or money is discussed. Votes from that
  /// same organization are shown apart: an org can't confirm itself.
  function discussionBlock(subject, { title = "Public discussion", empty = "Nobody has posted yet.", party = null } = {}) {
    const D = F.discussion(T, subject);
    if (!D.posts.length) return `<section class="card"><h2>${title}</h2><p class="muted small">${empty}</p></section>`;
    const latestIds = new Set(D.voices.map((v) => v.post.id));
    const voiceOf = Object.fromEntries(D.voices.map((v) => [v.author, v]));
    const partyOrg = party && T.orgs[party]?.orgId;
    const ownSide = (a) => same(a, party) || (partyOrg && T.orgs[a]?.orgId === partyOrg);
    const orgV = D.voices.filter((v) => verified(v.author) && !ownSide(v.author));
    const others = D.voices.filter((v) => !verified(v.author) && v.weight === 0);
    const count = (vs, s) => vs.filter((v) => Math.sign(v.stance) === s).length;
    const { support, dispute } = D.weighted;
    const lanes = `<div class="lanes">
      <div class="lane"><span class="small muted">Weighted by money given</span>
        <b class="num">${num(support)} for · ${num(dispute)} against</b>
        <div class="tug">${support + dispute > 0 ? `<i style="--c:var(--ok);flex:${support}"></i><i style="--c:var(--bad);flex:${dispute}"></i>` : ""}</div>
        <span class="small muted">What authors and their delegators gave that was paid out through signed milestones.</span></div>
      <div class="lane"><span class="small muted">Verified organizations</span><b class="num">${count(orgV, 1)} for · ${count(orgV, -1)} against</b><span class="small muted">Latest stance of each org${party ? ", not counting the one whose claim this is" : ""}.</span></div>
      <div class="lane"><span class="small muted">Other wallets</span><b class="num">${count(others, 1)} for · ${count(others, -1)} against</b><span class="small muted">No giving record and not verified. Shown, not counted.</span></div></div>`;
    const postHTML = (p) => {
      const v = voiceOf[p.author];
      const isLatest = latestIds.has(p.id);
      const d = doc(p.uri);
      const zero = !verified(p.author) && (v?.weight || 0) === 0;
      const mark = p.stance > 0 ? `<span class="mark yes" aria-label="supports">✓</span>` : p.stance < 0 ? `<span class="mark no" aria-label="disputes">✗</span>` : `<span class="mark zero" aria-label="comment">·</span>`;
      const weight = isLatest
        ? v.weight > 0
          ? `<span class="weight">Weight <b>${num(v.weight)}</b>${v.delegated > 0 ? ` (own ${num(v.own)} + ${v.delegators.length} who delegated: ${v.delegators.map((a) => esc(isYou(a) ? "you" : name(a))).join(", ")})` : ""}</span>`
          : `<span class="weight">${ownSide(p.author) ? "Their own claim: not counted" : zero ? "No giving record" : "No giving record, verified org"}</span>`
        : `<span class="weight">Earlier post, replaced by their latest</span>`;
      const photo = (d?.content.match(/ipfs:\/\/demo\/photo-[a-z0-9-]+/) || [])[0];
      return `<div class="post"${isLatest ? "" : ' style="opacity:.6"'}>${mark}<div class="stack" style="gap:2px">
        <span>${who(p.author)} <span class="small muted">${date(p.time)}</span></span>
        <span class="small">${esc(d?.content || "")}</span>
        <span class="row">${weight}${photo ? ` <button type="button" class="linkish small" data-doc="${esc(photo)}">See photo</button>` : ""}</span></div></div>`;
    };
    const main = D.posts.filter((p) => !(others.some((o) => o.author === p.author) && others.length > 2));
    const hidden = D.posts.filter((p) => !main.includes(p));
    return `<section class="card"><h2>${title}</h2>${lanes}<div class="stack">${main.map(postHTML).join("")}</div>
      ${hidden.length ? `<details><summary>${hidden.length} posts from ${others.length} wallets with no giving record</summary><div class="stack" style="margin-top:8px">${hidden.map(postHTML).join("")}</div></details>` : ""}
      <p class="small muted">Anyone can post with <code>forum.post(subject, 0, stance, uri, hash)</code>. Nothing here blocks money: it informs donors and curators, who decide where future money goes.</p></section>`;
  }

  // ------------------------------------------------------------- impact
  function viewImpact() {
    return `${demoNote}<section class="stack"><h1>Impact reports</h1>
      <p>Each report is tied to a milestone that was actually paid, and pins its photos and text by hash. If two reports claim the same spot or reuse a photo, both are flagged as contested and the argument happens in public.</p></section>
      <section class="grid2">${T.deeds
        .map((d) => {
          const ev = IX.events[d.event];
          const m = ev.milestones[d.milestoneId];
          return `<div class="card">${deedMini(d)}<p class="small muted">Paid ${money(m.amount, m.token)} by ${esc(ev.name)} to ${esc(name(m.payee))}.</p></div>`;
        })
        .join("")}</section>`;
  }

  function cellDiagram(d) {
    const latE5 = Math.round(d.lat * 1e5), lonE5 = Math.round(d.lon * 1e5);
    const cLat = Math.floor(latE5 / 10), cLon = Math.floor(lonE5 / 10);
    const S = 46, N = 5, ox = 10, oy = 10;
    const toXY = (la, lo) => [ox + (lo / 10 - (cLon - 2)) * S, oy + (cLat + 3 - la / 10) * S];
    let g = "";
    for (let r = 0; r < N; r++)
      for (let c = 0; c < N; c++) {
        const near = r >= 1 && r <= 3 && c >= 1 && c <= 3;
        const own = r === 2 && c === 2;
        g += `<rect x="${ox + c * S}" y="${oy + r * S}" width="${S}" height="${S}" fill="${own ? "var(--ok)" : near ? "var(--ok-soft)" : "none"}" fill-opacity="${own ? 0.35 : 1}" stroke="var(--line)"/>`;
      }
    for (const c of d.contested) {
      const o = T.deeds.find((x) => x.id === c.with);
      if (!o || c.reason !== "place") continue;
      const [bx, by] = toXY(Math.round(o.lat * 1e5), Math.round(o.lon * 1e5));
      g += `<path d="M${bx - 6} ${by - 6}L${bx + 6} ${by + 6}M${bx + 6} ${by - 6}L${bx - 6} ${by + 6}" stroke="var(--bad)" stroke-width="3"/>`;
    }
    const [x, y] = toXY(latE5, lonE5);
    g += `<circle cx="${x}" cy="${y}" r="6" fill="var(--ok)" stroke="var(--surface)" stroke-width="2"/>`;
    const W = ox * 2 + N * S;
    return `<svg viewBox="0 0 ${W + 150} ${W}" width="${W + 150}" role="img" aria-label="Map cells around this report">${g}
      <g transform="translate(${W + 4} 20)">
        <circle cx="6" cy="0" r="6" fill="var(--ok)"/><text x="18" y="4">This report</text>
        <rect x="0" y="18" width="12" height="12" fill="var(--ok-soft)" stroke="var(--line)"/><text x="18" y="28">Overlap zone</text>
        <path d="M0 46L12 58M12 46L0 58" stroke="var(--bad)" stroke-width="3"/><text x="18" y="56">Competing claim</text>
        <text x="0" y="86">1 cell ≈ 11 m</text></g></svg>`;
  }

  function fundingChain(ev, m) {
    const chain = [];
    let cur = ev;
    for (let guard = 0; cur && guard < 8; guard++) {
      const grant = T.events.flatMap((p) => p.milestones.filter((x) => x.payee === cur.address && x.status === "Released").map((x) => ({ p, x })))[0];
      chain.push({ ev: cur, grant });
      cur = grant?.p;
    }
    return `<div class="journey">${chain
      .map(({ ev: e, grant }, k) => {
        const mm = k === 0 ? m : null;
        return `<div class="hop"><div class="rail" style="--c:${eventColor(e.address)}"><span class="dot"></span><span class="line"></span></div><div class="hop-body">
          <div class="hop-title"><h3><a href="#event-${eventNo(e.address)}">${esc(e.name)}</a></h3></div>
          <p class="small">Run by ${who(e.organizer)}.${mm ? ` Paid ${money(mm.amount, mm.token)} to ${esc(name(mm.payee))} on ${date(mm.releasedAt)}.` : ""}</p>
          ${mm ? `<div class="sig">Signed off by ${sigChips(mm)}</div>` : ""}
          ${grant ? `<p class="small muted">Funded by a ${money(grant.x.amount, grant.x.token)} grant from ${esc(grant.p.name)}.</p>` : `<p class="small muted">Funded by ${e.donations.length} gifts.</p>`}
        </div></div>`;
      })
      .join("")}</div>`;
  }

  function viewDeed(i) {
    const d = T.deeds[i];
    if (!d) return `<p>Unknown report. <a href="#impact">See all reports</a>.</p>`;
    const ev = IX.events[d.event];
    const m = ev.milestones[d.milestoneId];
    const yourShare = F.trace(T, T.meta.you).flatMap((tr) => F.leaves(tr)).filter((l) => l.deed === d).reduce((s, l) => s + l.amount, 0);
    const osm = `https://www.openstreetmap.org/?mlat=${d.lat}&mlon=${d.lon}#map=16/${d.lat}/${d.lon}`;
    const contested = d.contested.length
      ? `<section class="card" style="border-color:var(--bad)"><h2>Contested</h2>${d.contested
          .map((c) => {
            const o = T.deeds.find((x) => x.id === c.with);
            return `<p class="small">${c.reason === "place" ? "Another report claims the same spot" : "Another report uses one of the same photos"}: <a href="#deed-${deedNo(o)}">${esc(deedName(o))}</a>, filed by ${who(o.reporter)} on ${date(o.time)}.</p>`;
          })
          .join("")}
        <p class="small muted">Neither report is blocked: a hard lock would let a fast copycat block the real builder. Read both discussions and decide.</p></section>`
      : "";
    const where = d.sensitive
      ? `<section class="card"><h2>Where</h2><p><span class="pill plain">Location protected</span></p>
          <p class="small">Published at ~11 km precision only (around ${d.lat.toFixed(1)}, ${d.lon.toFixed(1)}), and the partner's name is withheld. Signers checked the site privately. Protected reports are never matched by place.</p></section>`
      : `<section class="card"><h2>Where</h2><p class="mono">${d.lat.toFixed(5)}, ${d.lon.toFixed(5)}</p>
          <div class="cells">${cellDiagram(d)}</div>
          <p class="small">Any other “${esc(d.category)}” report in this cell or the 8 around it gets flagged as contested.</p>
          <p class="small"><a href="${osm}" rel="noopener">Open in OpenStreetMap</a></p></section>`;
    return `<p><a href="#impact">← All impact reports</a></p>${demoNote}
      <section class="stack"><h1>${esc(deedName(d))}</h1><div class="row">${deedPill(d)}</div>
        ${yourShare > 0.004 ? `<p class="lead" style="font-size:17px">Your gifts paid <strong>${money(yourShare, m.token)}</strong> of this ${money(m.amount, m.token)} job.</p>` : ""}</section>
      ${contested}
      <section class="card"><h2>Photos</h2><div class="gallery">${d.photos
        .map((p) => {
          const f = doc(p.uri);
          return `<figure><img src="${svgSrc(f.content)}" alt="${esc(f.title)}" data-photo="${esc(p.uri)}" data-hash="${esc(p.hash)}">
            <figcaption><span>${esc(f.title)}</span>${check(p.uri, p.hash)}</figcaption></figure>`;
        })
        .join("")}</div>
        <p class="small muted">Each photo's sha256 was written on-chain with the report. Your browser re-hashes the files above.</p></section>
      <div class="grid2">
        <section class="card"><h2>Report</h2><pre class="doc">${esc(doc(d.reportURI)?.content)}</pre>${check(d.reportURI, d.reportHash)}
          <p class="small">Filed by ${who(d.reporter)} on ${date(d.time)}.</p></section>
        ${where}
      </div>
      <section class="card"><h2>Who paid for it</h2>${fundingChain(ev, m)}</section>
      ${discussionBlock(d.id, { title: "Checked in public", party: d.reporter })}
      <section class="card"><h2>Check it yourself</h2>
        <ol class="stack" style="padding-left:20px;margin:0">
          <li>Download a photo or the report and compute its sha256. It must equal the hash shown on the file.</li>
          <li>Read the report from the ImpactRegistry contract with <code>deed(id)</code>.</li>
          <li>Been there? Post what you saw with <code>forum.post(id, 0, 1 or -1, note)</code>.</li>
        </ol>
        <dl class="kv"><dt>Report id</dt><dd class="mono">${d.id} <button class="btn" type="button" data-copy="${d.id}">Copy</button></dd>
          <dt>ImpactRegistry</dt><dd class="mono">${T.meta.contracts.impact}</dd><dt>Forum</dt><dd class="mono">${T.meta.contracts.forum}</dd></dl>
        <p class="small muted">In this demo the chain is a local test network, so these addresses don't exist on a public explorer.</p></section>`;
  }

  // ------------------------------------------------------------- events
  function poolBar(ev, t) {
    const x = ev.totals[t];
    const paid = x.released, held = x.committed - x.released, free = x.donated - x.committed;
    const seg = (v, c, l) => (v > 0.004 ? `<i style="--c:${c};flex:${v}" title="${l}: ${money(v, t)}"></i>` : "");
    return `<div class="stack" style="gap:6px"><div class="row between small"><span class="who">${money(x.donated, t)} received</span>
      <span class="muted">${money(paid, t)} paid · ${money(held, t)} waiting · ${money(free, t)} unspent</span></div>
      <div class="bar">${seg(paid, "var(--ok)", "Paid")}${seg(held, "var(--warn)", "Waiting for signatures")}${seg(free, "var(--muted)", "Unspent")}</div></div>`;
  }
  // New money per month, by kind of source.
  function inflowChart(ev) {
    const rows = {};
    const kinds = { grant: "Grants from other events", vault: "Monthly gifts", oneoff: "One-off gifts" };
    for (const d of ev.donations) {
      const k = IX.events[d.donor] ? "grant" : same(d.via, T.meta.contracts.vaults) ? "vault" : "oneoff";
      const m = monthKey(d.time);
      rows[m] = rows[m] || {};
      rows[m][k] = (rows[m][k] || 0) + d.amount;
    }
    const colors = { grant: "var(--c2)", vault: "var(--c1)", oneoff: "var(--c3)" };
    const keys = Object.keys(kinds).filter((k) => Object.values(rows).some((r) => r[k]));
    return `${monthBars(rows, keys, (k) => colors[k], { height: 130 })}<div class="legend">${keys.map((k) => `<span><i style="--c:${colors[k]}"></i>${kinds[k]}</span>`).join("")}</div>
      <p class="small muted">All stablecoins counted 1:1.</p>`;
  }

  function viewEvents() {
    return `${demoNote}<section class="stack"><h1>Events</h1><p>Each event is an escrow contract. Money leaves when enough independent, verified organizations sign a milestone. Nobody can freeze it; donors can send their next gifts elsewhere.</p></section>
      ${T.events
        .map((ev, i) => {
          const recent = ev.donations.filter((d) => d.time > T.meta.asOf - 92 * 86400 && !IX.events[d.donor]).reduce((s, d) => s + d.amount, 0);
          return `<section class="card"><div class="row between"><h2><a href="#event-${i}">${esc(ev.name)}</a></h2>${ev.closed ? `<span class="pill bad">Closed</span>` : `<span class="pill info">Open</span>`}</div>
          <p class="small">Run by ${who(ev.organizer)}. ${ev.threshold} of ${ev.attesters.length} signers needed. New money from donors in the last 3 months: <b>${num(recent)}</b>.</p>
          ${ev.tokens.filter((t) => ev.totals[t].donated > 0).map((t) => poolBar(ev, t)).join("")}
          <h3>New money per month</h3>${inflowChart(ev)}</section>`;
        })
        .join("")}`;
  }

  function viewEvent(i) {
    const ev = T.events[i];
    if (!ev) return `<p>Unknown event. <a href="#events">See all events</a>.</p>`;
    const top = [...ev.donations].sort((a, b) => b.amount - a.amount);
    const parents = T.events.filter((p) => p.milestones.some((m) => m.payee === ev.address));
    const ms = ev.milestones
      .map((m) => {
        const deed = IX.deeds[`${ev.address}:${m.id}`];
        const child = IX.events[m.payee];
        const status =
          m.status === "Released"
            ? `<span class="pill ok">Paid ${date(m.releasedAt)}</span>`
            : m.status === "Withdrawn"
              ? `<span class="pill bad">Withdrawn ${date(m.withdrawnAt)}</span>`
              : `<span class="pill warn">Waiting for signatures (${m.approvals} of ${ev.threshold})</span>`;
        const D = F.discussion(T, m.subject);
        const talk = !deed && D.posts.length ? `<details open><summary>Public discussion (${D.posts.length} posts)</summary>${discussionBlock(m.subject, { title: "Discussion", party: ev.organizer })}</details>` : "";
        return `<div class="ms"><div class="row between"><h3>${esc(budgetTitle(m))}</h3><span class="amount num">${money(m.amount, m.token)}</span></div>
          <div class="row">${status}<span class="small">to ${child ? `<a href="#event-${eventNo(child.address)}">${esc(child.name)}</a>` : who(m.payee)}</span>
          <button type="button" class="linkish small" data-doc="${esc(m.descriptionURI)}">Budget line</button></div>
          ${m.attestations.length ? `<div class="sig">Signed by ${sigChips(m)}</div>` : ""}
          ${m.withdrawReasonURI ? `<div class="note" style="--c:var(--bad)">${esc(doc(m.withdrawReasonURI)?.content || "")}</div>` : ""}
          ${talk}${deed ? deedMini(deed) : ""}</div>`;
      })
      .join("");
    return `<p><a href="#events">← All events</a></p>${demoNote}
      <section class="stack"><h1>${esc(ev.name)}</h1>
        <p>Run by ${who(ev.organizer)}. <button type="button" class="linkish" data-doc="${esc(ev.metadataURI)}">Read the charter</button></p>
        ${parents.length ? `<p class="small muted">Funded in part by ${parents.map((p) => `<a href="#event-${eventNo(p.address)}">${esc(p.name)}</a>`).join(", ")}.</p>` : ""}</section>
      <section class="card">${ev.tokens.filter((t) => ev.totals[t].donated > 0).map((t) => poolBar(ev, t)).join("")}
        <div class="sig">Signers (${ev.threshold} of ${ev.attesters.length} needed): ${ev.attesters.map((a) => `<span class="chip" style="cursor:default">${esc(name(a))}${verified(a) ? " ✓" : ""}</span>`).join("")}</div>
        <h3>New money per month</h3>${inflowChart(ev)}</section>
      <section class="card"><h2>Milestones</h2>${ms || `<p class="muted">None yet.</p>`}</section>
      ${discussionBlock(addressSubject(ev.organizer), { title: `What people say about ${esc(name(ev.organizer))}`, empty: "No posts about this organization.", party: ev.organizer })}
      <section class="card"><h2>Gifts</h2><p class="small">${ev.donations.length} gifts. Largest first; tap a donor to follow their money.</p>
        <div class="table-wrap"><table><thead><tr><th>Donor</th><th class="num">Amount</th><th>Date</th></tr></thead><tbody>
        ${top
          .slice(0, 8)
          .map((d) => `<tr><td>${IX.events[d.donor] ? esc(name(d.donor)) : `<button type="button" class="linkish" data-donor="${d.donor}">${esc(isYou(d.donor) ? "You" : name(d.donor))}</button>`}</td><td class="num">${money(d.amount, d.token)}</td><td>${date(d.time)}</td></tr>`)
          .join("")}</tbody></table></div></section>`;
  }

  // ----------------------------------------------------------- curators
  function curatorStats(c) {
    const followers = new Set();
    const latest = {};
    for (const r of T.vaults) if (r.type === "Followed" || r.type === "AllocationSet") if (!same(r.who, c)) latest[r.who] = r;
    for (const [w, r] of Object.entries(latest)) if (r.type === "Followed" && same(r.curator, c)) followers.add(w);
    const moved = {};
    for (const r of T.vaults)
      if (r.type === "Paid" && same(followingAt(r.who, r.time), c)) {
        const m = monthKey(r.time);
        moved[m] = moved[m] || {};
        moved[m][r.target] = (moved[m][r.target] || 0) + r.amount;
      }
    return { followers, moved };
  }

  function viewCurators() {
    const cs = curators();
    return `${demoNote}<section class="stack"><h1>Curators</h1>
      <p>You don't have to pick the perfect organization. Follow a curator, an evaluator who publishes a split across events, and your monthly gift follows their split. Curators compete for followers; switching takes one transaction.</p></section>
      ${cs
        .map((c, i) => {
          const { followers, moved } = curatorStats(c);
          const alloc = allocAt(c, T.meta.asOf);
          const dests = [...new Set(Object.values(moved).flatMap((r) => Object.keys(r)))];
          return `<section class="card"><div class="row between"><h2><a href="#curator-${i}">${esc(name(c))}</a></h2><span class="pill info">${followers.size} follower${followers.size === 1 ? "" : "s"}</span></div>
            ${splitHTML(alloc)}<h3>Followers' money moved per month</h3>${monthBars(moved, dests, eventColor, { height: 120 })}${legend(dests, eventColor)}</section>`;
        })
        .join("")}`;
  }

  function viewCurator(i) {
    const c = curators()[i];
    if (!c) return `<p>Unknown curator. <a href="#curators">See all curators</a>.</p>`;
    const { followers, moved } = curatorStats(c);
    const changes = T.vaults.filter((r) => r.type === "AllocationSet" && same(r.who, c));
    const dests = [...new Set(Object.values(moved).flatMap((r) => Object.keys(r)))];
    return `<p><a href="#curators">← All curators</a></p>${demoNote}
      <section class="stack"><h1>${esc(name(c))}</h1><p>${who(c)} · ${followers.size} donors follow this split${[...followers].some(isYou) ? ", including you" : ""}.</p></section>
      <section class="card"><h2>Current split</h2>${splitHTML(allocAt(c, T.meta.asOf))}</section>
      <section class="card"><h2>Split history</h2><div class="history">${changes
        .map((r) => `<div><span class="small muted">${date(r.time)}</span><br>${r.targets.map((x, k) => `${pct(r.bps[k])} ${esc(name(x))}`).join(", ")}</div>`)
        .join("")}</div></section>
      <section class="card"><h2>Followers' money moved per month</h2>${monthBars(moved, dests, eventColor)}${legend(dests, eventColor)}</section>
      ${discussionBlock(addressSubject(c), { title: "Notes and reasons", empty: "No notes yet.", party: c })}`;
  }

  // -------------------------------------------------------------- rules
  function viewRules() {
    const orgs = Object.entries(T.orgs);
    return `${demoNote}<section class="stack"><h1>How it stays honest</h1>
      <p>Nobody can freeze money. Instead, everything is public, and every donor can send their next gift elsewhere. Organizations compete for that money.</p></section>
      <section class="card"><h2>The levers</h2><ul class="stack" style="padding-left:20px;margin:0">
        <li><b>Exit.</b> Monthly gifts go out one month at a time. Redirecting, following a curator or withdrawing what's not given yet takes one transaction.</li>
        <li><b>Gates, not freezes.</b> A milestone pays out only when enough independent, verified organizations sign it. If they won't sign, the organizer withdraws it, as with the truck rental.</li>
        <li><b>Open argument.</b> Anyone can post on a milestone, a report or an organization, with evidence pinned by hash. Overlapping reports are flagged as contested, never silently blocked.</li>
        <li><b>Weight from money given.</b> A voice weighs what its author gave that was actually paid out through signed milestones. New wallets can speak but weigh nothing, so griefing costs real money.</li>
        <li><b>Delegation.</b> Lend your weight to an organization or evaluator you trust; take it back any time. Speaking yourself on a subject overrides it.</li>
        <li><b>Curators.</b> Evaluators publish splits; donors follow them. You don't need the perfect organization, just competition.</li></ul></section>
      <section class="card"><h2>Protecting the people doing the work</h2><ul class="stack" style="padding-left:20px;margin:0">
        <li>The record tracks money and organizations, never beneficiaries. Field staff act under their organization's identity.</li>
        <li>Reports can be marked sensitive: the location is stored at ~11 km precision and partner names are withheld, like the shelter tank.</li>
        <li>Everything here is permanent. Anything that could put someone at risk later must never be posted.</li>
        <li>Harassment is on the record too, and weighs nothing unless the harasser has given real money.</li></ul></section>
      <section class="card"><h2>Hard rules in the contracts</h2><ul class="stack" style="padding-left:20px;margin:0">
        <li>Organizers and signers must hold a verified identity from a trusted issuer.</li>
        <li>Signers must be different organizations from the organizer and from each other.</li>
        <li>Nobody signs money going to themselves, or into an event they run.</li>
        <li>A milestone can only earmark money that was donated and not yet committed.</li>
        <li>A report needs a paid milestone, and a milestone backs one report only.</li></ul>
        <h3>Refused so far</h3><div class="stack">${T.blocked
          .map(
            (b) => `<div class="blocked"><div class="stack" style="gap:2px"><span><span class="who">${esc(name(b.actor))}</span> <span class="small muted">on ${date(b.time)}</span></span>
            <span>Tried to: ${esc(b.action)}.</span><span class="small">Refused: ${esc(b.rule)}. <code>${esc(b.error)}</code></span></div></div>`
          )
          .join("")}</div></section>
      <section class="card"><h2>Verified organizations</h2><div class="table-wrap"><table><thead><tr><th>Organization</th><th>Registry entry</th><th>Verified by</th><th>Since</th></tr></thead><tbody>
        ${orgs.map(([a, o]) => `<tr><td>${who(a)}<div class="mono muted">${short(a)}</div></td><td class="mono">${esc(o.uri.split("/").pop())}</td><td>${esc(T.issuers[o.issuer]?.name)}</td><td>${date(o.issuedAt)}</td></tr>`).join("")}
        </tbody></table></div><p class="small muted">Two wallets above share one registry entry: they count as one organization.</p></section>
      <section class="card"><h2>Known limits</h2><ul class="stack" style="padding-left:20px;margin:0">
        <li>Money already given can't be pulled back, unless the organizer closes the event. Bob's EURC at Blue Horizon is unspent but stuck there.</li>
        <li>Weight from giving favours large donors. Square-root weighting would soften it but needs proof that each person is unique.</li>
        <li>Groups of organizations that sign for each other are visible here, but not prevented.</li>
        <li>Hashes prove a file is unchanged, not that it's true. Site visits by people with something at stake remain the real check.</li></ul></section>`;
  }

  // ------------------------------------------------------------- router
  function render() {
    const h = location.hash.slice(1) || "trace";
    let html, section, m;
    if ((m = /^deed-(\d+)$/.exec(h))) [html, section] = [viewDeed(+m[1]), "impact"];
    else if ((m = /^event-(\d+)$/.exec(h))) [html, section] = [viewEvent(+m[1]), "events"];
    else if ((m = /^curator-(\d+)$/.exec(h))) [html, section] = [viewCurator(+m[1]), "curators"];
    else if (h === "impact") [html, section] = [viewImpact(), "impact"];
    else if (h === "events") [html, section] = [viewEvents(), "events"];
    else if (h === "curators") [html, section] = [viewCurators(), "curators"];
    else if (h === "rules") [html, section] = [viewRules(), "rules"];
    else [html, section] = [viewTrace(), "trace"];
    app.innerHTML = html;
    document.querySelectorAll("nav a").forEach((a) => {
      if (a.getAttribute("href") === `#${section}`) a.setAttribute("aria-current", "page");
      else a.removeAttribute("aria-current");
    });
    const sel = document.getElementById("donor");
    if (sel)
      sel.addEventListener("change", () => {
        state.donor = sel.value;
        render();
      });
    runChecks(app);
  }
  window.addEventListener("hashchange", () => {
    render();
    window.scrollTo(0, 0);
  });
  render();
})();
