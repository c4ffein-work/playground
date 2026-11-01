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
  const sym = (t) => T.tokens[t]?.symbol || "";
  const nf2 = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
  const nf0 = new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 });
  const num = (v) => (Number.isInteger(v) && v >= 100 ? nf0.format(v) : nf2.format(v));
  const money = (v, t) => `${num(v)} ${sym(t)}`;
  const df = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" });
  const date = (ts) => (ts ? df.format(ts * 1000) : "");
  const name = (a) => T.orgs[a]?.name || T.labels[a] || IX.events[a]?.name || short(a);
  const isYou = (a) => a && a.toLowerCase() === T.meta.you.toLowerCase();
  const who = (a) => {
    const o = T.orgs[a];
    const tag = o ? (o.verified ? ` <span class="badge" title="Identity verified by ${esc(T.issuers[o.issuer]?.name)}">✓ verified</span>` : ` <span class="badge no">revoked</span>`) : "";
    return `<span class="who">${esc(name(a))}</span>${tag}`;
  };
  const doc = (uri) => T.docs[uri];
  const docTitle = (uri, fallback = "") => doc(uri)?.title || fallback;
  const cap = (x) => x.charAt(0).toUpperCase() + x.slice(1);
  const budgetTitle = (m) => cap(docTitle(m.descriptionURI, `Milestone #${m.id}`).replace(/^Budget line: /, ""));
  const svgSrc = (svg) => `data:image/svg+xml;charset=utf-8,${encodeURIComponent(svg)}`;
  const eventNo = (addr) => T.events.findIndex((e) => e.address === addr);
  const deedNo = (d) => T.deeds.indexOf(d);
  const deedPlace = (d) => docTitle(d.reportURI).split(", ").pop() || "";
  const categoryName = (c) => ({ "water-well": "Water well", latrine: "Latrine" })[c] || c;
  const deedName = (d) => `${categoryName(d.category)}, ${deedPlace(d)}`;
  const deedCover = (d) => doc(d.photos[d.photos.length - 1]?.uri)?.content;
  const tally = (d) => {
    const yes = d.votes.filter((v) => v.confirms);
    return { yes: yes.length, verifiedYes: yes.filter((v) => v.verified).length, no: d.votes.length - yes.length };
  };
  const deedPill = (d) => {
    const t = tally(d);
    if (t.no > 0) return `<span class="pill warn">Disputed by ${t.no}</span>`;
    if (t.verifiedYes > 0) return `<span class="pill ok">Confirmed on site</span>`;
    return `<span class="pill info">Reported</span>`;
  };
  const statusPill = (m) =>
    ({
      Released: `<span class="pill ok">Paid ${date(m.releasedAt)}</span>`,
      Approved: `<span class="pill warn">Challenge window until ${date(m.challengeDeadline)}</span>`,
      Proposed: `<span class="pill plain">Waiting for signatures</span>`,
      Disputed: `<span class="pill bad">Challenged</span>`,
      Rejected: `<span class="pill bad">Rejected</span>`,
    })[m.status];

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

  const state = { donor: T.meta.you };
  const demoNote = `<p class="demo-note">Demo data: fictional organizations on a local test chain, as of ${date(T.meta.asOf)}. Photos are illustrations, but their hashes are real and checked live below.</p>`;

  // -------------------------------------------------------------- trace
  const KIND = {
    deed: { label: "Built and publicly reported", c: "var(--ok)" },
    payment: { label: "Paid for services", c: "var(--accent)" },
    pending: { label: "Signed, not paid yet", c: "var(--warn)" },
    unspent: { label: "Not spent yet, in escrow", c: "var(--muted)" },
    refundable: { label: "Refundable (event cancelled)", c: "var(--bad)" },
  };
  const kindOf = (leaf) => (leaf.kind === "payment" ? (leaf.deed ? "deed" : "payment") : leaf.kind);

  function viewTrace() {
    const trees = F.trace(T, state.donor);
    const donors = F.donors(T);
    const options = donors
      .map((d) => {
        const label = isYou(d.address) ? "You" : name(d.address);
        return `<option value="${d.address}"${d.address === state.donor ? " selected" : ""}>${esc(label)}</option>`;
      })
      .join("");
    const donorName = isYou(state.donor) ? "You" : name(state.donor);

    let html = `${demoNote}<section class="hero">
      <div class="picker"><label for="donor">Follow the money of</label><select id="donor">${options}</select></div>`;
    if (!trees.length) return `${html}<p>No donations found for this wallet.</p></section>`;

    const sentences = trees
      .map((t) => `<strong>${money(t.amount, t.token)}</strong> to <a href="#event-${eventNo(t.event.address)}">${esc(t.event.name)}</a> on ${date(t.donatedAt)}`)
      .join(", and ");
    html += `<h1 class="lead">${esc(donorName)} gave ${sentences}. Here is where it went.</h1>`;

    for (const tree of trees) {
      const sums = {};
      for (const l of F.leaves(tree)) sums[kindOf(l)] = (sums[kindOf(l)] || 0) + l.amount;
      const keys = Object.keys(KIND).filter((k) => sums[k] > 0.004);
      html += `<div class="tiles">${keys
        .map((k) => `<div class="tile" style="--c:${KIND[k].c}"><b class="num">${money(sums[k], tree.token)}</b><span>${KIND[k].label}</span></div>`)
        .join("")}</div>
        <div class="bar" role="img" aria-label="Split of ${money(tree.amount, tree.token)}">${keys
          .map((k) => `<i style="--c:${KIND[k].c};flex:${sums[k]}" title="${KIND[k].label}: ${money(sums[k], tree.token)}"></i>`)
          .join("")}</div>`;
    }
    html += `</section>`;

    for (const tree of trees) {
      html += `<section class="card"><div class="row between"><h2>The path, org by org</h2><span class="scroll-hint">Scroll sideways on small screens</span></div>
        <div class="flow-wrap">${sankey(tree, donorName)}</div>
        <p class="small muted">Pooled money is shared out pro rata: your part of every payment is what you gave divided by the pool, times that payment. The same rule applies at every hop.</p></section>
        <section class="card"><h2>Step by step</h2><div class="journey">${hop(tree, true)}</div></section>`;
    }
    return html;
  }

  function sankey(tree, donorName) {
    const nodes = [];
    const push = (n, depth, parent) => {
      const node = { ...n, depth, parent, kids: [] };
      nodes.push(node);
      if (parent) parent.kids.push(node);
      return node;
    };
    const root = push({ label: donorName, sub: money(tree.amount, tree.token), value: tree.amount, c: "var(--accent)" }, 0, null);
    const walk = (n, depth, parent) => {
      if (n.kind === "event") {
        const me = push({ label: n.event.name, sub: `${money(n.amount, n.token)} · ${(n.share * 100).toFixed(2)}% of pool`, value: n.amount, c: "var(--accent)", href: `#event-${eventNo(n.event.address)}` }, depth, parent);
        n.children.forEach((c) => walk(c, depth + 1, me));
      } else {
        const k = kindOf(n);
        const label =
          k === "unspent" || k === "refundable"
            ? "Still in escrow"
            : n.deed
              ? deedName(n.deed)
              : `${name(n.milestone.payee)}`;
        const sub = k === "pending" ? `${money(n.amount, n.token)} · waiting` : money(n.amount, n.token);
        push({ label, sub, value: n.amount, c: KIND[k].c, href: n.deed ? `#deed-${deedNo(n.deed)}` : null }, depth, parent);
      }
    };
    walk(tree, 1, root);

    const cols = Math.max(...nodes.map((n) => n.depth)) + 1;
    const colW = 200, nodeW = 10, gap = 12, slot = 34, padY = 10;
    const scale = 120 / tree.amount;
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
    const W = 8 + (cols - 1) * colW + nodeW + 180;
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
    return `<svg width="${W}" height="${H}" viewBox="0 0 ${W} ${H}" role="img" aria-label="Flow of ${esc(donorName)}'s donation">${links}${marks}</svg>`;
  }

  const sigChips = (m) =>
    m.attestations
      .map((a) => `<button type="button" class="chip" data-doc="${esc(a.evidenceURI)}" data-hash="${esc(a.evidenceHash)}">${esc(name(a.attester))} · ${date(a.time)}</button>`)
      .join("");

  function deedMini(d, extra = "") {
    const t = tally(d);
    return `<a class="deed-mini" href="#deed-${deedNo(d)}">
      <img src="${svgSrc(deedCover(d))}" alt="Photo of ${esc(deedName(d))}">
      <span class="stack" style="gap:4px"><b>${esc(deedName(d))}</b>
      <span class="row">${deedPill(d)}<span class="small muted">${t.yes} confirmed · ${t.no} disputed</span></span>
      <span class="small muted">${d.photos.length} photos, report filed ${date(d.time)}${extra}</span></span></a>`;
  }

  function hop(n, isRoot) {
    const dot = (c) => `<div class="rail" style="--c:${c}"><span class="dot"></span><span class="line"></span></div>`;
    if (n.kind === "event") {
      const ev = n.event;
      const via = n.milestone
        ? `<p class="small muted">Granted by ${esc(n.parent.name)} as “${esc(budgetTitle(n.milestone))}”, paid ${date(n.milestone.releasedAt)}.</p>
           <div class="sig">Signed off by ${sigChips(n.milestone)}</div>`
        : "";
      const t = ev.totals[n.token];
      return `<div class="hop">${dot("var(--accent)")}<div class="hop-body">
        <div class="hop-title"><h3><a href="#event-${eventNo(ev.address)}">${esc(ev.name)}</a></h3><span class="amount num">${money(n.amount, n.token)}</span></div>
        <p class="small">Escrow run by ${who(ev.organizer)}. Your part: ${(n.share * 100).toFixed(2)}% of the ${money(t.donated, n.token)} it received.</p>
        ${via}
        <div class="indent journey">${n.children.map((c) => hop(c, false)).join("")}</div></div></div>`;
    }
    const k = kindOf(n);
    if (k === "unspent" || k === "refundable") {
      return `<div class="hop">${dot(KIND[k].c)}<div class="hop-body"><div class="hop-title"><h3>${KIND[k].label}</h3><span class="amount num">${money(n.amount, n.token)}</span></div>
        <p class="small muted">Held by the ${esc(n.event.name)} contract. It can only leave through a signed milestone${n.event.cancelled ? ", or as a refund to donors" : ""}.</p></div></div>`;
    }
    const m = n.milestone;
    const head = `<div class="hop-title"><h3>${esc(budgetTitle(m))}</h3><span class="amount num">${money(n.amount, n.token)}</span></div>`;
    if (k === "pending") {
      return `<div class="hop">${dot(KIND.pending.c)}<div class="hop-body">${head}
        <div class="row">${statusPill(m)}<span class="small muted">to ${esc(name(m.payee))}, ${m.approvals} of ${n.event.threshold} signatures</span></div>
        <div class="sig">${sigChips(m)}</div></div></div>`;
    }
    return `<div class="hop">${dot(KIND[k].c)}<div class="hop-body">${head}
      <p class="small">Paid to ${who(m.payee)} on ${date(m.releasedAt)}.</p>
      <div class="sig">Signed off by ${sigChips(m)}</div>
      ${n.deed ? deedMini(n.deed) : ""}</div></div>`;
  }

  // ------------------------------------------------------------- impact
  function viewImpact() {
    return `${demoNote}<section class="stack"><h1>Impact reports</h1>
      <p>Each report is tied to a milestone that was actually paid. It pins its photos and text by hash, and claims a spot on the map so nobody else can claim the same work.</p></section>
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
    const S = 46, N = 5, ox = 10, oy = 10; // 5x5 cells around the deed
    const toXY = (la, lo) => [ox + (lo / 10 - (cLon - 2)) * S, oy + ((cLat + 3) - la / 10) * S];
    let g = "";
    for (let r = 0; r < N; r++)
      for (let c = 0; c < N; c++) {
        const locked = r >= 1 && r <= 3 && c >= 1 && c <= 3;
        const own = r === 2 && c === 2;
        g += `<rect x="${ox + c * S}" y="${oy + r * S}" width="${S}" height="${S}" fill="${own ? "var(--ok)" : locked ? "var(--ok-soft)" : "none"}" fill-opacity="${own ? 0.35 : 1}" stroke="var(--line)"/>`;
      }
    const [x, y] = toXY(latE5, lonE5);
    g += `<circle cx="${x}" cy="${y}" r="6" fill="var(--ok)" stroke="var(--surface)" stroke-width="2"/>`;
    for (const b of T.blocked.filter((b) => b.deed === d.id && b.lat)) {
      const [bx, by] = toXY(Math.round(b.lat * 1e5), Math.round(b.lon * 1e5));
      g += `<path d="M${bx - 6} ${by - 6}L${bx + 6} ${by + 6}M${bx + 6} ${by - 6}L${bx - 6} ${by + 6}" stroke="var(--bad)" stroke-width="3"/>`;
    }
    const W = ox * 2 + N * S;
    return `<svg viewBox="0 0 ${W + 150} ${W}" width="${W + 150}" role="img" aria-label="Locked map cells around this deed">${g}
      <g transform="translate(${W + 4} 20)">
        <circle cx="6" cy="0" r="6" fill="var(--ok)"/><text x="18" y="4">This ${esc(categoryName(d.category).toLowerCase())}</text>
        <rect x="0" y="18" width="12" height="12" fill="var(--ok-soft)" stroke="var(--line)"/><text x="18" y="28">Locked cells</text>
        <path d="M0 46L12 58M12 46L0 58" stroke="var(--bad)" stroke-width="3"/><text x="18" y="56">Refused claim</text>
        <text x="0" y="86">1 cell ≈ 11 m</text></g></svg>`;
  }

  function viewDeed(i) {
    const d = T.deeds[i];
    if (!d) return `<p>Unknown report. <a href="#impact">See all reports</a>.</p>`;
    const ev = IX.events[d.event];
    const m = ev.milestones[d.milestoneId];
    const t = tally(d);

    // Funding chain: this event, then whoever granted into it.
    const chain = [];
    let cur = ev;
    for (let guard = 0; cur && guard < 8; guard++) {
      const parentGrant = T.events.flatMap((p) => p.milestones.filter((x) => x.payee === cur.address && x.status === "Released").map((x) => ({ p, x })))[0];
      chain.push({ ev: cur, grant: parentGrant });
      cur = parentGrant?.p;
    }
    const yourShare = F.trace(T, T.meta.you)
      .flatMap((tr) => F.leaves(tr))
      .filter((l) => l.deed === d)
      .reduce((s, l) => s + l.amount, 0);

    const blocked = T.blocked.filter((b) => b.deed === d.id);
    const votes = d.votes
      .map((v) => {
        const note = doc(v.noteURI);
        return `<div class="vote"><span class="mark ${v.confirms ? "yes" : "no"}" aria-hidden="true">${v.confirms ? "✓" : "✗"}</span>
          <div class="stack" style="gap:2px"><span>${who(v.voter)} <span class="small muted">${v.confirms ? "confirmed" : "disputed"} on ${date(v.time)}</span></span>
          ${note ? `<span class="small">${esc(note.content)}</span>` : ""}</div></div>`;
      })
      .join("");
    const linkedPhotos = d.votes
      .map((v) => (doc(v.noteURI)?.content.match(/ipfs:\/\/demo\/photo-[a-z0-9-]+/) || [])[0])
      .filter(Boolean);

    const osm = `https://www.openstreetmap.org/?mlat=${d.lat}&mlon=${d.lon}#map=16/${d.lat}/${d.lon}`;
    return `<p><a href="#impact">← All impact reports</a></p>
      ${demoNote}
      <section class="stack"><h1>${esc(deedName(d))}</h1>
        <div class="row">${deedPill(d)}<span class="small muted">${t.yes} confirmations (${t.verifiedYes} from verified orgs) · ${t.no} disputes</span></div>
        ${yourShare > 0 ? `<p class="lead" style="font-size:17px">Your donation paid <strong>${money(yourShare, m.token)}</strong> of this ${money(m.amount, m.token)} job.</p>` : ""}
      </section>

      <section class="card"><h2>Photos</h2><div class="gallery">${d.photos
        .map((p) => {
          const f = doc(p.uri);
          return `<figure><img src="${svgSrc(f.content)}" alt="${esc(f.title)}" data-photo="${esc(p.uri)}" data-hash="${esc(p.hash)}">
            <figcaption><span>${esc(f.title)}</span>${check(p.uri, p.hash)}</figcaption></figure>`;
        })
        .join("")}</div>
        <p class="small muted">Each photo's sha256 was written on-chain with the report. Your browser re-hashes the files above. A photo can only ever back one report.</p></section>

      <div class="grid2">
        <section class="card"><h2>Report</h2><pre class="doc">${esc(doc(d.reportURI)?.content)}</pre>${check(d.reportURI, d.reportHash)}
          <p class="small">Filed by ${who(d.reporter)} on ${date(d.time)}.</p></section>
        <section class="card"><h2>Where</h2>
          <p class="mono">${d.lat.toFixed(5)}, ${d.lon.toFixed(5)}</p>
          <div class="cells">${cellDiagram(d)}</div>
          <p class="small">The report claims its map cell and the 8 around it for “${esc(d.category)}”. Another claim there is refused on-chain${blocked.some((b) => b.lat) ? ", which already happened once (red cross)" : ""}.</p>
          <p class="small"><a href="${osm}" rel="noopener">Open in OpenStreetMap</a></p></section>
      </div>

      <section class="card"><h2>Who paid for it</h2><div class="journey">${chain
        .map(({ ev: e, grant }, k) => {
          const mm = k === 0 ? m : null;
          return `<div class="hop"><div class="rail" style="--c:var(--accent)"><span class="dot"></span><span class="line"></span></div><div class="hop-body">
            <div class="hop-title"><h3><a href="#event-${eventNo(e.address)}">${esc(e.name)}</a></h3></div>
            <p class="small">Run by ${who(e.organizer)}.${mm ? ` Paid ${money(mm.amount, mm.token)} to ${esc(name(mm.payee))} on ${date(mm.releasedAt)}.` : ""}</p>
            ${mm ? `<div class="sig">Signed off by ${sigChips(mm)}</div>` : ""}
            ${grant ? `<p class="small muted">Funded by a ${money(grant.x.amount, grant.x.token)} grant from ${esc(grant.p.name)}, signed by ${grant.x.attestations.map((a) => esc(name(a.attester))).join(" and ")}.</p>` : `<p class="small muted">Funded by ${e.donations.length} donations.</p>`}
          </div></div>`;
        })
        .join("")}</div></section>

      <section class="card"><h2>Checked by people on site</h2><div class="votes">${votes || `<p class="muted">No checks yet.</p>`}</div>
        ${linkedPhotos.map((u) => `<button type="button" class="linkish small" data-doc="${esc(u)}">See the photo attached to the dispute</button>`).join("")}</section>

      ${blocked.length ? `<section class="card"><h2>Duplicate claims refused</h2>${blocked.map(blockedRow).join("")}</section>` : ""}

      <section class="card"><h2>Check it yourself</h2>
        <ol class="stack" style="padding-left:20px;margin:0">
          <li>Download a photo or the report and compute its sha256. It must equal the hash shown on the file.</li>
          <li>Read the deed from the ImpactRegistry contract with <code>deed(id)</code> and compare.</li>
          <li>Been there? Call <code>vote(id, true or false, note)</code> from your wallet. Verified organizations are counted separately.</li>
        </ol>
        <dl class="kv"><dt>Deed id</dt><dd class="mono">${d.id} <button class="btn" type="button" data-copy="${d.id}">Copy</button></dd>
          <dt>ImpactRegistry</dt><dd class="mono">${T.meta.contracts.impact} <button class="btn" type="button" data-copy="${T.meta.contracts.impact}">Copy</button></dd>
          <dt>Report tx</dt><dd class="mono">${short(d.tx)}, block ${d.block}</dd></dl>
        <p class="small muted">In this demo the chain is a local test network, so the addresses don't exist on a public explorer.</p></section>`;
  }

  // ------------------------------------------------------------- events
  function poolBar(ev, t) {
    const x = ev.totals[t];
    const paid = x.released, held = x.committed - x.released, free = x.donated - x.committed;
    const seg = (v, c, l) => (v > 0 ? `<i style="--c:${c};flex:${v}" title="${l}: ${money(v, t)}"></i>` : "");
    return `<div class="stack" style="gap:6px"><div class="row between small"><span class="who">${money(x.donated, t)} received</span>
      <span class="muted">${money(paid, t)} paid · ${money(held, t)} signed, waiting · ${money(free, t)} free</span></div>
      <div class="bar">${seg(paid, "var(--ok)", "Paid")}${seg(held, "var(--warn)", "Waiting")}${seg(free, "var(--muted)", "Free")}</div></div>`;
  }

  function viewEvents() {
    return `${demoNote}<section class="stack"><h1>Events</h1><p>Each event is an escrow contract. Money leaves only through milestones that independent, verified organizations have signed, after a public challenge window.</p></section>
      ${T.events
        .map(
          (ev, i) => `<section class="card"><div class="row between"><h2><a href="#event-${i}">${esc(ev.name)}</a></h2>${ev.cancelled ? `<span class="pill bad">Cancelled</span>` : `<span class="pill info">Open</span>`}</div>
          <p class="small">Run by ${who(ev.organizer)}. ${ev.threshold} of ${ev.attesters.length} signers needed, ${Math.round(ev.challengeWindow / 86400)}-day challenge window.</p>
          ${ev.tokens.filter((t) => ev.totals[t].donated > 0).map((t) => poolBar(ev, t)).join("")}</section>`
        )
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
        const notes = [
          ...m.challenges.map((c) => ({ c: "var(--bad)", head: `Challenged by ${esc(name(c.by))} on ${date(c.time)}`, uri: c.reasonURI })),
          ...m.rulings.map((r) => ({ c: r.upheld ? "var(--ok)" : "var(--bad)", head: `Ruling on ${date(r.time)}: ${r.upheld ? "payment upheld" : "payment rejected"}`, uri: r.reasonURI })),
        ]
          .map((n) => `<div class="note" style="--c:${n.c}"><b>${n.head}.</b> ${esc(doc(n.uri)?.content || "")}</div>`)
          .join("");
        return `<div class="ms"><div class="row between"><h3>${esc(budgetTitle(m))}</h3><span class="amount num">${money(m.amount, m.token)}</span></div>
          <div class="row">${statusPill(m)}<span class="small">to ${child ? `<a href="#event-${eventNo(child.address)}">${esc(child.name)}</a>` : who(m.payee)}</span>
          <button type="button" class="linkish small" data-doc="${esc(m.descriptionURI)}">Budget line</button></div>
          ${m.attestations.length ? `<div class="sig">Signed by ${sigChips(m)}</div>` : ""}
          ${notes}${deed ? deedMini(deed) : ""}</div>`;
      })
      .join("");
    return `<p><a href="#events">← All events</a></p>${demoNote}
      <section class="stack"><h1>${esc(ev.name)}</h1>
        <p>Run by ${who(ev.organizer)}. Disputes go to ${who(ev.arbiter)}.
        <button type="button" class="linkish" data-doc="${esc(ev.metadataURI)}">Read the charter</button></p>
        ${parents.length ? `<p class="small muted">Funded in part by ${parents.map((p) => `<a href="#event-${eventNo(p.address)}">${esc(p.name)}</a>`).join(", ")}.</p>` : ""}</section>
      <section class="card">${ev.tokens.filter((t) => ev.totals[t].donated > 0).map((t) => poolBar(ev, t)).join("")}
        <div class="sig">Signers (${ev.threshold} of ${ev.attesters.length} needed): ${ev.attesters.map((a) => `<span class="chip" style="cursor:default">${esc(name(a))}${T.orgs[a]?.verified ? " ✓" : ""}</span>`).join("")}</div></section>
      <section class="card"><h2>Milestones</h2>${ms || `<p class="muted">None yet.</p>`}</section>
      <section class="card"><h2>Donations</h2><p class="small">${ev.donations.length} donations. Largest first; tap one to follow it.</p>
        <div class="table-wrap"><table><thead><tr><th>Donor</th><th class="num">Amount</th><th>Date</th></tr></thead><tbody>
        ${top
          .slice(0, 8)
          .map((d) => `<tr><td>${IX.events[d.donor] ? esc(name(d.donor)) : `<button type="button" class="linkish" data-donor="${d.donor}">${esc(isYou(d.donor) ? "You" : name(d.donor))}</button>`}</td><td class="num">${money(d.amount, d.token)}</td><td>${date(d.time)}</td></tr>`)
          .join("")}</tbody></table></div></section>`;
  }

  // -------------------------------------------------------------- rules
  function blockedRow(b) {
    const deed = T.deeds.find((d) => d.id === b.deed);
    return `<div class="blocked"><div class="stack" style="gap:2px">
      <span><span class="who">${esc(name(b.actor))}</span> <span class="small muted">on ${date(b.time)}</span></span>
      <span>Tried to: ${esc(b.action)}.</span>
      <span class="small">Refused: ${esc(b.rule)}. <code>${esc(b.error)}</code>${deed ? ` · <a href="#deed-${deedNo(deed)}">${esc(deedName(deed))}</a>` : ""}</span></div></div>`;
  }

  function viewRules() {
    const orgs = Object.entries(T.orgs);
    return `${demoNote}<section class="stack"><h1>Safeguards</h1><p>These rules are enforced by the contracts, so they can't be skipped. Below: every attempt the demo made to break them, and who is verified.</p></section>
      <section class="card"><h2>Refused by the rules</h2><div class="stack">${T.blocked.map(blockedRow).join("")}</div></section>
      <section class="card"><h2>What the contracts enforce</h2><ul class="stack" style="padding-left:20px;margin:0">
        <li>Organizers, signers and arbiters must hold a verified identity from a trusted issuer.</li>
        <li>Signers must be different organizations from the organizer and from each other. Several wallets of one org count once.</li>
        <li>Nobody signs off money going to themselves, or into an event they run.</li>
        <li>A milestone can only earmark money that was donated and not yet committed.</li>
        <li>After sign-off, donors and signers have a challenge window. A challenge freezes the payment until the arbiter rules.</li>
        <li>An impact report needs a paid milestone. It locks its map spot, its photos and its milestone, so the same work can't be claimed twice.</li>
        <li>If an event is cancelled, donors get back their share of what wasn't paid out.</li></ul></section>
      <section class="card"><h2>Verified organizations</h2><div class="table-wrap"><table><thead><tr><th>Organization</th><th>Registry entry</th><th>Verified by</th><th>Since</th></tr></thead><tbody>
        ${orgs.map(([a, o]) => `<tr><td>${who(a)}<div class="mono muted">${short(a)}</div></td><td class="mono">${esc(o.uri.split("/").pop())}</td><td>${esc(T.issuers[o.issuer]?.name)}</td><td>${date(o.issuedAt)}</td></tr>`).join("")}
        </tbody></table></div>
        <p class="small muted">Two wallets above share one registry entry: they count as one organization.</p></section>`;
  }

  // ------------------------------------------------------------- router
  function render() {
    const h = location.hash.slice(1) || "trace";
    let html, section;
    let m;
    if ((m = /^deed-(\d+)$/.exec(h))) [html, section] = [viewDeed(+m[1]), "impact"];
    else if ((m = /^event-(\d+)$/.exec(h))) [html, section] = [viewEvent(+m[1]), "events"];
    else if (h === "impact") [html, section] = [viewImpact(), "impact"];
    else if (h === "events") [html, section] = [viewEvents(), "events"];
    else if (h === "rules") [html, section] = [viewRules(), "rules"];
    else [html, section] = [viewTrace(), "trace"];
    app.innerHTML = html;
    document.querySelectorAll("nav a").forEach((a) => a.toggleAttribute("aria-current", a.getAttribute("href") === `#${section}`));
    document.querySelectorAll("nav a[aria-current]").forEach((a) => a.setAttribute("aria-current", "page"));
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
