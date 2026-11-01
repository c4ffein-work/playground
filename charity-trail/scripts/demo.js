// Plays a full 8-month story on the local Hardhat chain, then writes the
// tracker site's data file (site/data.js).
//   npx hardhat run scripts/demo.js
// All organizations, people and places are fictional demo data.
const fs = require("fs");
const path = require("path");
const { ethers, network } = require("hardhat");
const { photo } = require("./lib/photos");
const { buildSnapshot } = require("./lib/snapshot");
const { printTrail } = require("./lib/trail");

const usd = (n) => ethers.parseUnits(Number(n).toFixed(6), 6);
const sha = (s) => ethers.sha256(ethers.toUtf8Bytes(s));
const DAY = 24 * 3600;
const WINDOW = 7 * DAY;

let clock = 0;
async function at(iso) {
  const t = Math.floor(Date.parse(iso) / 1000);
  clock = Math.max(t, clock + 1);
  await network.provider.send("evm_setNextBlockTimestamp", [clock]);
}

// Off-chain documents (stand-ins for IPFS files). The chain only stores
// their sha256; the site re-hashes them to prove they weren't altered.
const docs = {};
function doc(slug, title, content, type = "text") {
  const uri = `ipfs://demo/${slug}`;
  docs[uri] = { title, type, content };
  return { uri, hash: sha(content) };
}
const pic = (slug, title, kind, caption, seed) => doc(slug, title, photo({ kind, caption, seed }), "svg");

// The story is written as dated steps, then run in date order.
const plan = [];
const step = (iso, fn) => plan.push({ t: Date.parse(iso), iso, fn });

const blocked = [];
async function attempt(info, fn, iface) {
  try {
    await fn();
    throw new Error(`expected "${info.action}" to be refused`);
  } catch (e) {
    if (e.message.startsWith("expected")) throw e;
    let name = /custom error '([A-Za-z]+)/.exec(e.message)?.[1];
    if (!name && e.data) {
      try {
        name = iface.parseError(e.data).name;
      } catch {}
    }
    const deed = typeof info.deed === "function" ? info.deed() : info.deed;
    blocked.push({ ...info, deed, error: name || e.shortMessage || "reverted", time: clock });
  }
}

async function main() {
  const s = await ethers.getSigners();
  const [admin, issuer, blue, kayes, village, audit, ombud, dune, dune2, quick, drilling, pumpworks, truckco, lab, you, bob, diaspora, duneContractor] = s;

  const labels = {};
  const label = (signer, name) => (labels[signer.address] = name);
  label(admin, "Registry admin");
  label(issuer, "Demo Charity Register");
  label(quick, "Quick Relief Org");
  label(drilling, "Sahel Drilling Co.");
  label(pumpworks, "HandPump Works");
  label(truckco, "Fast Freight Ltd.");
  label(lab, "Bamako Water Lab");
  label(you, "You");
  label(bob, "Bob");
  label(diaspora, "Kayes diaspora fund");
  label(duneContractor, "Dune Builders");

  // ------------------------------------------------------------ setup
  await at("2026-03-02T09:00:00Z");
  const identity = await (await ethers.getContractFactory("IdentityRegistry")).deploy();
  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy(await identity.getAddress());
  const impact = await (await ethers.getContractFactory("ImpactRegistry")).deploy(
    await registry.getAddress(),
    await identity.getAddress()
  );
  const Token = await ethers.getContractFactory("MockStablecoin");
  const usdc = await Token.deploy("USD Coin", "USDC");
  const eurc = await Token.deploy("Euro Coin", "EURC");
  const U = await usdc.getAddress();
  const E = await eurc.getAddress();

  await identity.setIssuer(issuer.address, true, "Demo Charity Register");
  const orgs = [
    [blue, "Blue Horizon Foundation", "REG-BHF-0192"],
    [kayes, "Kayes Water Collective", "REG-KWC-2231"],
    [village, "Village A & B councils", "REG-VAB-0007"],
    [audit, "Sahel Audit Cooperative", "REG-SAC-1180"],
    [ombud, "Water Ombudsman Panel", "REG-WOP-0003"],
    [dune, "Dune Aid", "REG-DUN-4410"],
    [dune2, "Dune Aid (field wallet)", "REG-DUN-4410"],
  ];
  for (const [signer, name, regId] of orgs) {
    label(signer, name);
    await identity.connect(issuer).issue(signer.address, ethers.id(regId), name, `https://register.example/${regId}`);
  }

  const created = async (tx) => {
    const r = await (await tx).wait();
    const l = r.logs.map((x) => registry.interface.parseLog(x)).find((x) => x?.name === "EventCreated");
    return ethers.getContractAt("CharityEvent", l.args.eventAddress);
  };

  await at("2026-03-02T10:00:00Z");
  const charter1 = doc(
    "charter-clean-water-kayes",
    "Event charter: Clean Water for Kayes 2026",
    "Goal: safe drinking water for two villages in the Kayes region.\nBudget: 10,000 USDC + logistics in EURC.\nPlan: fund a local partner to drill and equip two boreholes, test water quality, keep spares.\nSigners: village councils, Sahel Audit Cooperative, Kayes Water Collective. 2 of 3 must sign.\nDisputes: Water Ombudsman Panel. Challenge window: 7 days."
  );
  const e1 = await created(
    registry
      .connect(blue)
      .createEvent("Clean Water for Kayes 2026", charter1.uri, ombud.address, [village.address, audit.address, kayes.address], 2, WINDOW, [U, E])
  );
  const charter2 = doc(
    "charter-wells-kayes",
    "Event charter: Wells for Village A & B",
    "Run by Kayes Water Collective with a grant from Blue Horizon Foundation.\nScope: two boreholes with hand pumps, plus a spare-parts kit.\nSigners: village councils and Sahel Audit Cooperative, both must sign.\nDisputes: Water Ombudsman Panel. Challenge window: 7 days."
  );
  const e2 = await created(
    registry
      .connect(kayes)
      .createEvent("Wells for Village A & B", charter2.uri, ombud.address, [village.address, audit.address], 2, WINDOW, [U])
  );
  const e1a = await e1.getAddress();
  const e2a = await e2.getAddress();
  labels[e1a] = "Clean Water for Kayes 2026 (escrow)";
  labels[e2a] = "Wells for Village A & B (escrow)";

  // ------------------------------------------------------- donations
  const fund = async (signer, token, amount, ev) => {
    await token.mint(signer.address, usd(amount));
    await token.connect(signer).approve(await ev.getAddress(), usd(amount));
    await ev.connect(signer).donate(await token.getAddress(), usd(amount));
  };

  // A crowd of small donors, so "your" 20 USDC is one drop in the pool.
  const crowd = [];
  for (let i = 0; i < 24; i++) {
    const w = ethers.Wallet.createRandom().connect(ethers.provider);
    await admin.sendTransaction({ to: w.address, value: ethers.parseEther("1") });
    crowd.push(w);
  }
  const amounts = [250, 40, 500, 75, 1200, 30, 60, 900, 150, 25, 400, 100, 2000, 50, 35, 300, 80, 120, 650, 45, 200, 90, 15];
  amounts.push(10000 - 20 - amounts.reduce((a, b) => a + b, 0)); // pool totals 10,000 USDC
  const day0 = Date.parse("2026-03-03T08:00:00Z");
  step("2026-03-04T19:42:00Z", () => fund(you, usdc, 20, e1));
  crowd.forEach((w, i) => step(new Date(day0 + i * 1.6 * DAY * 1000).toISOString(), () => fund(w, usdc, amounts[i], e1)));
  let deedA, deedB, e3, e3a; // set by earlier steps, used by later ones
  step("2026-03-10T12:00:00Z", async () => {
    await fund(bob, eurc, 3000, e1);
  });
  step("2026-04-02T15:00:00Z", async () => {
    await fund(diaspora, usdc, 1000, e2);
  });
  // ------------------------------------------------ blocked: identity
  step("2026-03-20T11:00:00Z", async () => {
    await attempt(
      {
        actor: quick.address,
        action: "Open an event with two of its own wallets as signers",
        rule: "Organizers and signers must hold a verified identity",
      },
      () => registry.connect(quick).createEvent("Quick water", "", ombud.address, [quick.address, s[19].address], 1, WINDOW, [U]),
      registry.interface
    );
  });
  step("2026-03-21T11:00:00Z", async () => {
    await attempt(
      {
        actor: dune.address,
        action: "List its own field wallet as an independent signer",
        rule: "Signers must be different organizations from the organizer",
      },
      () => registry.connect(dune).createEvent("Dune water points", "", ombud.address, [dune2.address, audit.address], 2, WINDOW, [U]),
      registry.interface
    );
  });
  // ------------------------------------------- grant: parent -> child
  const sign = async (ev, id, signer, slug, title, content, when) => {
    await at(when);
    const d = doc(slug, title, content);
    await ev.connect(signer).attest(id, d.hash, d.uri);
  };

  step("2026-04-20T09:00:00Z", async () => {
    const grant = doc(
      "budget-grant-kwc",
      "Budget line: grant to Kayes Water Collective",
      "9,000 USDC to the event 'Wells for Village A & B' (escrow contract).\nCovers 2 boreholes (4,200 each) and a spare-parts kit (600)."
    );
    await e1.connect(blue).proposeMilestone(grant.uri, e2a, U, usd(9000));
    await sign(e1, 0, village, "att-grant-village", "Village councils: support letter", "Both village councils requested boreholes at the sites in the siting survey. We support this grant.", "2026-04-21T10:00:00Z");
    await sign(e1, 0, audit, "att-grant-audit", "Sahel Audit: partner due diligence", "Kayes Water Collective: registered since 2019, 2023-2025 accounts reviewed, no findings. Drilling quotes in line with regional prices.", "2026-04-22T16:00:00Z");
  });
  step("2026-04-30T09:00:00Z", async () => {
    await e1.release(0);
  });
  // -------------------------------------------- Well A (child event)
  const survey = pic("photo-survey", "Siting survey map", "survey", "Siting survey · Kayes region · 2026-04-12", 3);
  step("2026-05-02T09:00:00Z", async () => {
    const wellA = doc("budget-well-a", "Budget line: borehole, Village A", "Drill a 60 m borehole and install a hand pump at Village A.\nContractor: Sahel Drilling Co. Quote: 4,200 USDC (attached to auditor note).");
    await e2.connect(kayes).proposeMilestone(wellA.uri, drilling.address, U, usd(4200));
    await sign(e2, 0, village, "att-wella-village", "Village A council: site agreed", `Site agreed with the council, 120 m from the school. Survey: ${survey.uri}`, "2026-05-03T10:00:00Z");
    await sign(e2, 0, audit, "att-wella-audit", "Sahel Audit: quote check", "Quote SDC-2026-044 checked: 4,200 USDC for 60 m with casing and pump, within the 3,900 to 4,600 regional range.", "2026-05-04T15:00:00Z");
  });
  step("2026-05-12T09:00:00Z", async () => {
    await e2.release(0);
  });
  // -------------------------------------------- Well B (child event)
  step("2026-05-20T09:00:00Z", async () => {
    const wellB = doc("budget-well-b", "Budget line: borehole, Village B", "Drill a 55 m borehole and install a hand pump at Village B.\nContractor: Sahel Drilling Co. Quote: 4,200 USDC.");
    await e2.connect(kayes).proposeMilestone(wellB.uri, drilling.address, U, usd(4200));
    await sign(e2, 1, village, "att-wellb-village", "Village B council: site agreed", "Site agreed, next to the market square.", "2026-05-21T10:00:00Z");
    await sign(e2, 1, audit, "att-wellb-audit", "Sahel Audit: quote check", "Quote SDC-2026-051 checked: 4,200 USDC, within the regional range.", "2026-05-22T15:00:00Z");
  });
  step("2026-06-02T09:00:00Z", async () => {
    await e2.release(1);
  });
  // ------------------------------------- logistics: challenged, rejected
  step("2026-05-05T09:00:00Z", async () => {
    const trucks = doc("budget-trucks", "Budget line: truck rental", "Truck rental to move drilling equipment, 2 trips: 2,500 EURC to Fast Freight Ltd.");
    await e1.connect(blue).proposeMilestone(trucks.uri, truckco.address, E, usd(2500));
    await sign(e1, 1, kayes, "att-trucks-kwc", "Kayes Water Collective: trucks needed", "Equipment must reach both villages before the rains.", "2026-05-06T10:00:00Z");
    await sign(e1, 1, audit, "att-trucks-audit", "Sahel Audit: invoice received", "Invoice FF-8812 received.", "2026-05-07T10:00:00Z");
  });
  step("2026-05-09T18:30:00Z", async () => {
    const ch = doc("challenge-trucks", "Challenge by a donor", "Two trips on this route cost about 800 EUR at local rates. This quote is about 3 times the market price. Also, Sahel Drilling's quote already includes transport.");
    await e1.connect(bob).challenge(1, ch.uri);
  });
  step("2026-05-14T12:00:00Z", async () => {
    const ruling = doc("ruling-trucks", "Ombudsman ruling", "Challenge upheld: transport is already in the drilling quote (SDC-2026-044, line 7). Payment rejected; 2,500 EURC returns to the pool.");
    await e1.connect(ombud).resolveDispute(1, false, ruling.uri);
  });
  // -------------------------------------------------- Well A report
  const pA = [
    pic("photo-wella-drilling", "Well A: drilling", "drilling", "Village A · 2026-05-27 10:14 · 14.45012, -11.44031", 11),
    pic("photo-wella-pump", "Well A: pump installed", "pump", "Village A · 2026-06-10 16:02 · 14.45012, -11.44031", 12),
    pic("photo-wella-water", "Well A: first water", "water", "Village A · 2026-06-12 08:30 · 14.45012, -11.44031", 13),
  ];
  const repA = doc(
    "report-well-a",
    "Completion report: borehole, Village A",
    [
      "Borehole completed on 2026-06-12, 62 m deep, static water level 31 m.",
      "Flow test: 1.4 m3/h over 6 hours. Hand pump: India Mark II.",
      "Serves about 640 people (council count), 120 m from the school.",
      "Paid: 4,200 USDC from 'Wells for Village A & B', milestone #0.",
      `Photos (sha256 on-chain): ${pA.map((p) => p.uri).join(", ")}`,
      "Filed by: Sahel Drilling Co.",
    ].join("\n")
  );
  step("2026-06-14T09:00:00Z", async () => {
    await impact.connect(drilling).reportDeed(e2a, 0, "water-well", 1445012, -1144031, repA.uri, repA.hash, pA.map((p) => p.hash), pA.map((p) => p.uri));
    deedA = await impact.deedIds(0);
  });
  // -------------------------------------------------- Well B report
  const pB = [
    pic("photo-wellb-drilling", "Well B: drilling", "drilling", "Village B · 2026-06-20 09:51 · 14.46230, -11.41875", 21),
    pic("photo-wellb-pump", "Well B: pump installed", "pump", "Village B · 2026-07-02 15:20 · 14.46230, -11.41875", 22),
  ];
  const repB = doc(
    "report-well-b",
    "Completion report: borehole, Village B",
    [
      "Borehole completed on 2026-07-02, 55 m deep, static water level 28 m.",
      "Flow test: 1.1 m3/h over 6 hours. Hand pump: India Mark II.",
      "Serves about 410 people, next to the market square.",
      "Paid: 4,200 USDC from 'Wells for Village A & B', milestone #1.",
      `Photos (sha256 on-chain): ${pB.map((p) => p.uri).join(", ")}`,
      "Filed by: Sahel Drilling Co.",
    ].join("\n")
  );
  step("2026-07-08T09:00:00Z", async () => {
    await impact.connect(drilling).reportDeed(e2a, 1, "water-well", 1446230, -1141875, repB.uri, repB.hash, pB.map((p) => p.hash), pB.map((p) => p.uri));
    deedB = await impact.deedIds(1);
  });
  // --------------------------------------------- public verification
  const visitors = [];
  for (let i = 0; i < 4; i++) {
    const w = ethers.Wallet.createRandom().connect(ethers.provider);
    await admin.sendTransaction({ to: w.address, value: ethers.parseEther("1") });
    visitors.push(w);
  }
  ["Visitor (teacher, Village A)", "Visitor (health worker)", "Visitor (traveller)", "Visitor (Village B resident)"].forEach(
    (n, i) => (labels[visitors[i].address] = n)
  );
  // `deed` is a function: deed ids only exist once their step has run.
  const vote = (who, deed, yes, slug, title, text, when, extra) => {
    const d = extra || doc(slug, title, text);
    step(when, () => impact.connect(who).vote(deed(), yes, d.uri));
  };
  vote(visitors[0], () => deedA, true, "vote-a-teacher", "Field note", "Pump works. Children fill bottles at break time.", "2026-06-18T12:00:00Z");
  vote(audit, () => deedA, true, "vote-a-audit", "Sahel Audit: site visit", "Site visit on 2026-06-20. Depth log, pump model and flow match the report. Water sample sent to the lab.", "2026-06-20T17:00:00Z");
  vote(village, () => deedA, true, "vote-a-council", "Village A council", "The well is in use every day.", "2026-06-25T10:00:00Z");
  vote(visitors[1], () => deedA, true, "vote-a-health", "Field note", "Visited on 2026-07-15, queue of about 15 people, water clear.", "2026-07-15T14:00:00Z");
  vote(visitors[2], () => deedB, true, "vote-b-traveller", "Field note", "Saw the pump in use at the market square.", "2026-07-20T09:00:00Z");
  const broken = pic("photo-wellb-broken", "Well B: pump handle broken", "broken", "Village B · 2026-10-11 07:55 · 14.46230, -11.41875", 31);
  vote(
    visitors[3],
    () => deedB,
    false,
    null,
    null,
    null,
    "2026-10-12T08:00:00Z",
    doc("vote-b-resident", "Dispute: pump broken", `Pump handle broke on 2026-10-09; nobody can draw water. Photo: ${broken.uri}`)
  );

  // ------------------------------- blocked: double-claiming the well
  const fresh = pic("photo-dune-wella", "Dune Aid photo at Well A", "pump", "Village A · 2026-07-24 · 14.45019, -11.44037", 41);
  step("2026-07-15T09:00:00Z", async () => {
    e3 = await created(
      registry.connect(dune).createEvent("Dune Aid water points", "", ombud.address, [village.address, audit.address], 2, WINDOW, [U])
    );
    e3a = await e3.getAddress();
  });
  step("2026-07-15T10:00:00Z", async () => {
    await fund(crowd[0], usdc, 500, e3);
    await e3.connect(dune).proposeMilestone("ipfs://demo/dune-works", duneContractor.address, U, usd(500));
    await e3.connect(village).attest(0, sha("dune-ok"), "");
    await e3.connect(audit).attest(0, sha("dune-ok"), "");
  });
  step("2026-07-23T09:00:00Z", async () => {
    await e3.release(0);
  });
  step("2026-07-25T09:00:00Z", async () => {
    await attempt(
      {
        actor: dune.address,
        action: "Report Well A (8 m away, new photo) as its own work",
        rule: "One deed per place: the ~11 m map cell and its neighbours are locked",
      lat: 14.45019,
      lon: -11.44037,
        deed: () => deedA,
      },
      () => impact.connect(dune).reportDeed(e3a, 0, "water-well", 1445019, -1144037, "", sha("x"), [fresh.hash], [fresh.uri]),
      impact.interface
    );
  });
  step("2026-07-25T09:30:00Z", async () => {
    await attempt(
      {
        actor: dune.address,
        action: "Report a well 3 km away, reusing a Well A photo",
        rule: "A photo can back one deed only (sha256 is locked)",
        deed: () => deedA,
      },
      () => impact.connect(dune).reportDeed(e3a, 0, "water-well", 1447800, -1144031, "", sha("y"), [pA[1].hash], [pA[1].uri]),
      impact.interface
    );
  });
  // ---------------------------------------------- water testing (E1)
  step("2026-09-01T09:00:00Z", async () => {
    const labLine = doc("budget-lab", "Budget line: water quality tests", "Lab tests for both boreholes (bacteria, nitrates, arsenic, fluoride): 400 USDC to Bamako Water Lab.");
    await e1.connect(blue).proposeMilestone(labLine.uri, lab.address, U, usd(400));
    await sign(e1, 2, kayes, "att-lab-kwc", "Kayes Water Collective: samples sent", "Samples from both wells sent on 2026-08-28.", "2026-09-02T10:00:00Z");
    await sign(e1, 2, audit, "att-lab-audit", "Sahel Audit: lab results", "Both wells pass WHO guidelines. E. coli: 0 CFU/100 mL.", "2026-09-03T10:00:00Z");
  });
  step("2026-09-12T09:00:00Z", async () => {
    await e1.release(2);
  });
  // ------------------------------------------- spares: in window now
  step("2026-10-20T09:00:00Z", async () => {
    const spares = doc("budget-spares", "Budget line: spare parts and repair", "Spare parts kit for both pumps (handles, seals, rods) and repair of the Village B handle: 600 USDC to HandPump Works.");
    await e2.connect(kayes).proposeMilestone(spares.uri, pumpworks.address, U, usd(600));
    await sign(e2, 2, village, "att-spares-village", "Village B council: repair needed", "The Village B pump handle broke on 2026-10-09.", "2026-10-21T10:00:00Z");
    await sign(e2, 2, audit, "att-spares-audit", "Sahel Audit: quote check", "Quote HPW-311: 600 USDC, in line with catalogue prices.", "2026-10-22T10:00:00Z");
  });
  step("2026-10-27T18:00:00Z", () => network.provider.send("evm_mine"));

  for (const { iso, fn } of plan.sort((a, b) => a.t - b.t)) {
    await at(iso);
    await fn();
  }

  // ------------------------------------------------------- output
  await printTrail(e1, labels);
  await printTrail(e2, labels);

  const snapshot = await buildSnapshot({
    identity,
    registry,
    impact,
    labels,
    docs,
    blocked,
    meta: { network: "Local demo chain (Hardhat)", generatedBy: "scripts/demo.js", you: you.address },
  });
  const out = path.join(__dirname, "..", "site", "data.js");
  fs.writeFileSync(out, `// Generated by scripts/demo.js. Do not edit.\nwindow.TRAIL = ${JSON.stringify(snapshot, null, 1)};\n`);
  console.log(`\nwrote ${path.relative(process.cwd(), out)} (${(fs.statSync(out).size / 1024).toFixed(0)} KiB), ${blocked.length} blocked attempts`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
