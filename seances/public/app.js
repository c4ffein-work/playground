// Séances — tiny vanilla front-end over /api/state.
(() => {
  const $ = (s, r = document) => r.querySelector(s);
  const h = (tag, attrs = {}, ...kids) => {
    const el = document.createElement(tag);
    for (const [k, v] of Object.entries(attrs)) {
      if (k === "class") el.className = v;
      else if (k.startsWith("on")) el.addEventListener(k.slice(2), v);
      else if (v !== false && v != null) el.setAttribute(k, v === true ? "" : v);
    }
    for (const k of kids.flat()) if (k != null && k !== false) el.append(k.nodeType ? k : document.createTextNode(k));
    return el;
  };

  let S = null; // last /api/state
  const filters = { day: "", cinema: 0, version: "", mine: false };
  const openSyn = new Set();
  let scrollRefresh = null;

  const api = async (path, body) => {
    const res = await fetch(path, body === undefined ? {} : { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });
    const data = await res.json().catch(() => ({}));
    if (!res.ok) throw new Error(data.error || res.statusText);
    return data;
  };
  const toast = (msg, ms = 2200) => {
    const t = h("div", { class: "toast" }, msg);
    document.body.append(t);
    setTimeout(() => t.remove(), ms);
  };

  const fmtDay = new Intl.DateTimeFormat("fr-FR", { weekday: "short", day: "numeric", month: "short" });
  const dayLabel = (iso) => {
    const [y, m, d] = iso.split("-").map(Number);
    return fmtDay.format(new Date(y, m - 1, d));
  };
  const shortCinema = (name) => name.replace(/^UGC (Ciné Cité )?/i, "");
  const initials = (name) => (name || "?").trim().split(/\s+/).map((w) => w[0]).slice(0, 2).join("");
  const userById = (id) => S.users.find((u) => u.id === id);
  const avatar = (id, lg) => {
    const u = userById(id);
    const me = S.me && id === S.me.id;
    return h("span", { class: `avatar${me ? " me" : ""}${lg ? " lg" : ""}`, title: u?.name || (me ? "you" : "someone") }, initials(u?.name || (me ? "me" : "?")));
  };
  const ago = (t) => {
    if (!t) return "never";
    const m = Math.round((Date.now() - t) / 60000);
    return m < 1 ? "just now" : m < 60 ? `${m} min ago` : m < 1440 ? `${Math.round(m / 60)} h ago` : `${Math.round(m / 1440)} d ago`;
  };

  // ---------- rendering ----------
  function render() {
    const app = $("#app");
    app.replaceChildren();
    $("#me-btn").textContent = S.me?.name ? S.me.name : "Who are you?";
    $("#refresh").disabled = S.refreshing || !S.city.id;
    $("#refresh-label").textContent = S.refreshing ? "scraping…" : "refresh";

    if (!S.city.id) {
      const node = $("#t-setup").content.cloneNode(true);
      const sel = $("#city-select", node);
      for (const c of S.cities) sel.append(h("option", { value: c.id }, c.name));
      sel.addEventListener("change", async () => {
        if (!sel.value) return;
        sel.disabled = true;
        try { S = await api("/api/city", { regionId: Number(sel.value) }); toast(`${S.city.name}: ${S.cinemas.length} cinemas, scraping…`); poll(); }
        catch (e) { toast(e.message, 4000); sel.disabled = false; return; }
        render();
      });
      app.append(node);
      return;
    }

    app.append(renderToolbar());
    if (!S.me?.name) app.append(h("section", { class: "card" },
      h("h2", {}, "Tell your friends who you are"),
      h("p", { class: "muted" }, "Pick a name before voting so your initials show up on the séances."),
      h("div", { class: "row" }, h("button", { class: "primary", onclick: openMe }, "Set my name"))));
    app.append(renderPlans());
    app.append(renderFilms());
  }

  function renderToolbar() {
    const days = [...new Set(S.showings.map((s) => s.date))];
    const versions = [...new Set(S.showings.map((s) => s.version))].sort();
    const bar = h("div", { class: "toolbar" });
    const citySel = h("select", { onchange: async (e) => {
      if (!e.target.value || Number(e.target.value) === S.city.id) return;
      if (!confirm("Changing city clears every vote. Continue?")) { e.target.value = S.city.id; return; }
      try { S = await api("/api/city", { regionId: Number(e.target.value) }); poll(); render(); } catch (err) { toast(err.message, 4000); }
    } });
    for (const c of S.cities) citySel.append(h("option", { value: c.id, selected: c.id === S.city.id }, c.name));
    bar.append(citySel);
    for (const c of S.cinemas) bar.append(h("button", { class: `chip${c.selected ? " on" : ""}`, title: c.selected ? "click to hide this cinema" : "click to include this cinema",
      onclick: async () => { S = await api("/api/cinema", { id: c.id, selected: !c.selected }); render(); } }, shortCinema(c.name)));
    bar.append(h("span", { class: "sep" }));
    bar.append(h("button", { class: `chip${filters.day === "" ? " on" : ""}`, onclick: () => { filters.day = ""; render(); } }, "all days"));
    for (const d of days) bar.append(h("button", { class: `chip${filters.day === d ? " on" : ""}`, onclick: () => { filters.day = filters.day === d ? "" : d; render(); } }, dayLabel(d)));
    if (versions.length > 1) {
      bar.append(h("span", { class: "sep" }));
      for (const v of versions) bar.append(h("button", { class: `chip${filters.version === v ? " on" : ""}`, onclick: () => { filters.version = filters.version === v ? "" : v; render(); } }, v));
    }
    bar.append(h("span", { class: "sep" }));
    bar.append(h("button", { class: `chip${filters.mine ? " on" : ""}`, onclick: () => { filters.mine = !filters.mine; render(); } }, "my picks"));
    const members = h("div", { class: "members" }, h("span", { class: "muted", style: "font-size:12px" }, `${S.users.length} member${S.users.length === 1 ? "" : "s"}:`), ...S.users.map((u) => avatar(u.id)));
    const lr = S.lastRefresh;
    const status = h("div", { class: "status" },
      `${S.showings.length} upcoming séances · programme scraped ${ago(S.lastScrape)}`,
      lr?.errors?.length ? h("span", { style: "color:var(--no)" }, ` · ${lr.errors.length} scrape error${lr.errors.length > 1 ? "s" : ""}: ${lr.errors[0]}`) : null);
    return h("div", {}, bar, members, status);
  }

  function visible(s) {
    return (!filters.day || s.date === filters.day) && (!filters.version || s.version === filters.version) &&
      (!filters.cinema || s.cinema_id === filters.cinema) && (!filters.mine || (S.me && s.available.includes(S.me.id)));
  }

  function renderPlans() {
    const byId = new Map(S.showings.map((s) => [s.id, s]));
    const films = new Map(S.films.map((f) => [f.id, f]));
    const cin = new Map(S.cinemas.map((c) => [c.id, c.name]));
    const plans = S.plans.map((id) => byId.get(id)).filter(Boolean);
    const sec = h("section", { class: "card plans" }, h("h2", {}, "Best plans"));
    if (!plans.length) { sec.append(h("p", { class: "empty" }, "No one has ticked a séance yet. Tap the ones you could make below.")); return sec; }
    sec.append(h("ol", {}, ...plans.map((s) => h("li", {},
      null,
      h("div", {},
        h("div", { class: "when" }, `${dayLabel(s.date)} ${s.time} · ${films.get(s.film_id)?.title ?? "?"}`),
        h("div", { class: "muted", style: "font-size:13px" }, `${shortCinema(cin.get(s.cinema_id) ?? "")} · ${s.version}${s.room ? " · " + s.room : ""}${s.end_time ? " · ends " + s.end_time : ""}`),
        h("div", { class: "who" }, ...s.available.map((id) => avatar(id)))),
      h("div", {},
        h("div", { style: "text-align:right;font-weight:800;font-size:18px" }, s.available.length, h("span", { class: "muted", style: "font-size:11px;font-weight:400" }, "/" + Math.max(S.users.length, 1))),
        h("a", { href: s.booking_url, target: "_blank", rel: "noopener", style: "font-size:12px" }, "book ↗"))))));
    return sec;
  }

  function renderFilms() {
    const me = S.me?.id;
    const cin = new Map(S.cinemas.map((c) => [c.id, c.name]));
    const multiCinema = S.cinemas.filter((c) => c.selected).length > 1;
    const byFilm = new Map();
    for (const s of S.showings) if (visible(s)) (byFilm.get(s.film_id) ?? byFilm.set(s.film_id, []).get(s.film_id)).push(s);
    const net = (f) => Object.values(f.votes).reduce((a, b) => a + b, 0);
    const films = S.films.filter((f) => byFilm.has(f.id)).sort((a, b) => net(b) - net(a) || a.title.localeCompare(b.title));
    const grid = h("section", { class: "films" });
    if (!films.length) grid.append(h("p", { class: "muted" }, S.showings.length ? "Nothing matches these filters." : S.refreshing ? "Scraping the programme…" : "No programme yet — hit refresh."));
    for (const f of films) {
      const mine = me ? f.votes[me] ?? 0 : 0;
      const ups = Object.entries(f.votes).filter(([, v]) => v > 0).map(([id]) => id);
      const downs = Object.entries(f.votes).filter(([, v]) => v < 0).map(([id]) => id);
      const vote = (v) => async () => { if (!me || !S.me.name) return openMe(); S = await api("/api/film-vote", { filmId: f.id, vote: mine === v ? 0 : v }); render(); };
      const syn = h("p", { class: `syn${openSyn.has(f.id) ? " open" : ""}`, onclick: () => { openSyn.has(f.id) ? openSyn.delete(f.id) : openSyn.add(f.id); render(); } }, f.synopsis);
      const days = h("div", { class: "days" });
      const byDay = new Map();
      for (const s of byFilm.get(f.id)) (byDay.get(s.date) ?? byDay.set(s.date, []).get(s.date)).push(s);
      for (const [d, list] of byDay) {
        days.append(h("div", { class: "day" }, h("span", { class: "d" }, dayLabel(d)), ...list.map((s) => {
          const isMe = me && s.available.includes(me);
          return h("button", { class: `slot${isMe ? " me" : s.available.length ? " some" : ""}`, title: `${cin.get(s.cinema_id)} · ${s.room || ""} · ends ${s.end_time || "?"}`,
            onclick: async () => { if (!me || !S.me.name) return openMe(); S = await api("/api/availability", { showingId: s.id, on: !isMe }); render(); } },
            s.time, h("span", { class: "v" }, s.version), multiCinema ? h("span", { class: "cin" }, shortCinema(cin.get(s.cinema_id) ?? "")) : null,
            s.available.length ? h("span", { class: "n" }, s.available.length) : null);
        })));
      }
      grid.append(h("article", { class: "card film" },
        f.poster ? h("img", { src: f.poster, alt: "", loading: "lazy" }) : h("div", { class: "noposter" }),
        h("div", {},
          h("h3", { class: "title" }, f.title),
          h("div", { class: "meta" }, f.label ? h("span", { class: "label" }, f.label) : null, [f.genre, f.duration, f.rating != null ? `★ ${f.rating}` : null].filter(Boolean).join(" · ")),
          f.synopsis ? syn : null,
          h("div", { class: "thumbs" },
            h("button", { class: `up${mine > 0 ? " on" : ""}`, onclick: vote(1), title: ups.map((id) => userById(id)?.name).join(", ") }, `👍 ${ups.length}`),
            h("button", { class: `down${mine < 0 ? " on" : ""}`, onclick: vote(-1), title: downs.map((id) => userById(id)?.name).join(", ") }, `👎 ${downs.length}`),
            h("span", { class: "net" }, net(f) > 0 ? `+${net(f)}` : net(f) < 0 ? `${net(f)}` : ""))),
        days));
    }
    return grid;
  }

  // ---------- me dialog ----------
  const dlg = $("#me-dialog");
  function openMe() {
    const f = $("#me-form");
    f.name.value = S.me?.name ?? ""; f.email.value = S.me?.email ?? ""; f.phone.value = S.me?.phone ?? "";
    $("#me-error").hidden = true; $("#login-error").hidden = true; $("#login-form").contact.value = "";
    dlg.showModal();
  }
  $("#me-btn").addEventListener("click", openMe);
  $("#me-cancel").addEventListener("click", () => dlg.close());
  $("#me-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    const f = e.target;
    try { S = await api("/api/me", { name: f.name.value, email: f.email.value, phone: f.phone.value }); dlg.close(); render(); }
    catch (err) { $("#me-error").textContent = err.message; $("#me-error").hidden = false; }
  });
  $("#login-form").addEventListener("submit", async (e) => {
    e.preventDefault();
    try { S = await api("/api/login", { contact: e.target.contact.value }); dlg.close(); toast(`Welcome back, ${S.me.name}`); render(); }
    catch (err) { $("#login-error").textContent = err.message; $("#login-error").hidden = false; }
  });
  $("#refresh").addEventListener("click", async () => { await api("/api/refresh", {}); S.refreshing = true; render(); poll(); });

  // ---------- live-ish ----------
  async function load() { S = await api("/api/state"); render(); }
  function poll() {
    clearTimeout(scrollRefresh);
    scrollRefresh = setTimeout(async () => {
      if (document.visibilityState === "visible" && !dlg.open) { try { await load(); } catch {} }
      poll();
    }, S?.refreshing ? 3000 : 20000);
  }
  document.addEventListener("visibilitychange", () => { if (document.visibilityState === "visible") load().catch(() => {}); });
  load().then(poll).catch((e) => { $("#app").textContent = "Cannot reach the server: " + e.message; });
})();
