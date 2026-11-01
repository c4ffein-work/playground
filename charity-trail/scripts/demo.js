// Plays an 8-month story on the local Hardhat chain, then writes the
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
const abi = ethers.AbiCoder.defaultAbiCoder();
const subjectOfMilestone = (ev, id) => ethers.keccak256(abi.encode(["address", "uint256"], [ev, id]));
const subjectOfAddress = (a) => ethers.zeroPadValue(a, 32);
const ZERO = ethers.ZeroHash;

let clock = 0;
async function at(t) {
  const last = (await ethers.provider.getBlock("latest")).timestamp;
  clock = Math.max(Math.floor(t / 1000), last + 1);
  await network.provider.send("evm_setNextBlockTimestamp", [clock]);
}

// The story is written as dated steps, then run in date order.
const plan = [];
const step = (iso, fn) => plan.push({ t: Date.parse(iso), n: plan.length, fn });

// Off-chain documents (stand-ins for IPFS files). The chain only stores
// their sha256; the site re-hashes them to prove they weren't altered.
const docs = {};
function doc(slug, title, content, type = "text") {
  const uri = `ipfs://demo/${slug}`;
  docs[uri] = { title, type, content };
  return { uri, hash: sha(content) };
}
const pic = (slug, title, kind, caption, seed) => doc(slug, title, photo({ kind, caption, seed }), "svg");

const blocked = [];
async function attempt(info, fn) {
  try {
    await fn();
    throw new Error(`expected "${info.action}" to be refused`);
  } catch (e) {
    if (e.message.startsWith("expected")) throw e;
    const name = /custom error '([A-Za-z]+)/.exec(e.message)?.[1];
    blocked.push({ ...info, error: name || e.shortMessage || "reverted", time: clock });
  }
}

async function wallet(admin) {
  const w = ethers.Wallet.createRandom().connect(ethers.provider);
  await admin.sendTransaction({ to: w.address, value: ethers.parseEther("1") });
  return w;
}

async function main() {
  const s = await ethers.getSigners();
  const [admin, issuer, blue, kayes, village, audit, sahelWatch, giveWise, dune, dune2, quick, drilling, you, bob, diaspora] = s;
  const truckco = ethers.Wallet.createRandom().address;
  const lab = ethers.Wallet.createRandom().address;
  const pumpworks = ethers.Wallet.createRandom().address;
  const guild = ethers.Wallet.createRandom().address;
  const shelter = ethers.Wallet.createRandom().address;
  const duneBuilders = ethers.Wallet.createRandom().address;
  const builder = ethers.Wallet.createRandom().address;

  const labels = {};
  const label = (a, name) => (labels[a.address || a] = name);
  label(admin, "Keeper bot");
  label(issuer, "Demo Charity Register");
  label(quick, "Quick Relief Org");
  label(drilling, "Sahel Drilling Co.");
  label(pumpworks, "HandPump Works");
  label(truckco, "Fast Freight Ltd.");
  label(lab, "Bamako Water Lab");
  label(guild, "Kayes Pump Mechanics Guild");
  label(shelter, "Shelter partner (name withheld)");
  label(duneBuilders, "Dune Builders");
  label(builder, "Village A builders");
  label(you, "You");
  label(bob, "Bob");
  label(diaspora, "Kayes diaspora fund");

  // ------------------------------------------------------------ setup
  await at(Date.parse("2026-03-02T09:00:00Z"));
  const identity = await (await ethers.getContractFactory("IdentityRegistry")).deploy();
  const registry = await (await ethers.getContractFactory("EventRegistry")).deploy(await identity.getAddress());
  const R = await registry.getAddress();
  const impact = await (await ethers.getContractFactory("ImpactRegistry")).deploy(R);
  const vaults = await (await ethers.getContractFactory("GivingVaults")).deploy(R);
  const forum = await (await ethers.getContractFactory("Forum")).deploy();
  const Token = await ethers.getContractFactory("MockStablecoin");
  const usdc = await Token.deploy("USD Coin", "USDC");
  const eurc = await Token.deploy("Euro Coin", "EURC");
  const U = await usdc.getAddress();
  const E = await eurc.getAddress();
  const V = await vaults.getAddress();

  await identity.setIssuer(issuer.address, true, "Demo Charity Register");
  const orgs = [
    [blue, "Blue Horizon Foundation", "REG-BHF-0192"],
    [kayes, "Kayes Water Collective", "REG-KWC-2231"],
    [village, "Village A & B councils", "REG-VAB-0007"],
    [audit, "Sahel Audit Cooperative", "REG-SAC-1180"],
    [sahelWatch, "Sahel Watch (evaluator)", "REG-SWE-0415"],
    [giveWise, "GiveWise Mali (evaluator)", "REG-GWM-0921"],
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

  const charter1 = doc(
    "charter-clean-water-kayes",
    "Event charter: Clean Water for Kayes 2026",
    "Goal: safe drinking water for two villages in the Kayes region.\nPlan: fund a local partner to drill and equip two boreholes, test water quality, support local services.\nSigners: village councils, Sahel Audit Cooperative, Kayes Water Collective. 2 of 3 must sign."
  );
  const e1 = await created(registry.connect(blue).createEvent("Clean Water for Kayes 2026", charter1.uri, [village.address, audit.address, kayes.address], 2, [U, E]));
  const charter2 = doc(
    "charter-wells-kayes",
    "Event charter: Wells for Village A & B",
    "Run by Kayes Water Collective. Scope: two boreholes with hand pumps, spares, mechanic training, school latrines.\nSigners: village councils and Sahel Audit Cooperative, both must sign."
  );
  const e2 = await created(registry.connect(kayes).createEvent("Wells for Village A & B", charter2.uri, [village.address, audit.address], 2, [U, E]));
  const e1a = await e1.getAddress();
  const e2a = await e2.getAddress();
  labels[e1a] = "Clean Water for Kayes 2026 (escrow)";
  labels[e2a] = "Wells for Village A & B (escrow)";

  const say = async (who, subject, stance, slug, title, text) => {
    const d = doc(slug, title, text);
    await forum.connect(who).post(subject, ZERO, stance, d.uri, d.hash);
  };

  // ------------------------------------------------------- curators
  await vaults.connect(sahelWatch).setAllocation([e1a, e2a], [5000, 5000]);
  await say(sahelWatch, subjectOfAddress(sahelWatch.address), 0, "sw-alloc-1", "Sahel Watch: March allocation", "50% Clean Water for Kayes, 50% Wells for Village A & B. We favour local organizations once they have a track record.");
  await vaults.connect(giveWise).setAllocation([e1a], [10000]);
  await say(giveWise, subjectOfAddress(giveWise.address), 0, "gw-alloc-1", "GiveWise Mali: allocation", "100% Clean Water for Kayes: Blue Horizon has 12 years of audited accounts.");

  // ------------------------------------------------------ one-off gifts
  const fund = async (w, token, amount, ev) => {
    await token.mint(w.address, usd(amount));
    await token.connect(w).approve(await ev.getAddress(), usd(amount));
    await ev.connect(w).donate(await token.getAddress(), usd(amount));
  };
  const crowd = [];
  for (let i = 0; i < 24; i++) crowd.push(await wallet(admin));
  const amounts = [250, 40, 500, 75, 1200, 30, 60, 900, 150, 25, 400, 100, 2000, 50, 35, 300, 80, 120, 650, 45, 200, 90, 15];
  amounts.push(9000 - amounts.reduce((a, b) => a + b, 0));
  const day0 = Date.parse("2026-03-03T08:00:00Z");
  crowd.forEach((w, i) => step(new Date(day0 + i * 1.6 * 86400 * 1000).toISOString(), () => fund(w, usdc, amounts[i], e1)));
  step("2026-03-10T12:00:00Z", () => fund(bob, eurc, 3000, e1));
  step("2026-04-02T15:00:00Z", () => fund(diaspora, usdc, 1000, e2));

  // --------------------------------------------- recurring gifts (vaults)
  const recurring = []; // [wallet, token]
  const setupVault = async (w, token, monthly, setup) => {
    await token.mint(w.address, usd(monthly * 8));
    await token.connect(w).approve(V, usd(monthly * 8));
    await vaults.connect(w).deposit(await token.getAddress(), usd(monthly * 8));
    await vaults.connect(w).setMonthly(await token.getAddress(), usd(monthly));
    await setup();
    recurring.push([w, await token.getAddress()]);
  };
  step("2026-03-03T07:00:00Z", async () => {
    await setupVault(you, usdc, 20, () => vaults.connect(you).setAllocation([e1a], [10000]));
    await setupVault(bob, eurc, 300, () => vaults.connect(bob).setAllocation([e1a], [10000]));
    await setupVault(diaspora, usdc, 150, () => vaults.connect(diaspora).setAllocation([e2a], [10000]));
    const monthly = [50, 100, 150, 75, 200, 120];
    for (let i = 0; i < 6; i++) {
      const w = crowd[i];
      await setupVault(w, usdc, monthly[i], () =>
        i < 3 ? vaults.connect(w).follow(sahelWatch.address) : i === 3 ? vaults.connect(w).follow(giveWise.address) : vaults.connect(w).setAllocation([e1a], [10000])
      );
    }
    await forum.connect(crowd[0]).delegate(sahelWatch.address);
    await forum.connect(crowd[1]).delegate(sahelWatch.address);
  });
  for (const month of ["03", "04", "05", "06", "07", "08", "09", "10"]) {
    step(`2026-${month}-04T08:00:00Z`, async () => {
      for (const [w, token] of recurring) await vaults.connect(admin).pay(w.address, token);
    });
  }

  // ------------------------------------------------ blocked: identity
  step("2026-03-20T11:00:00Z", () =>
    attempt(
      { actor: quick.address, action: "Open an event with two of its own wallets as signers", rule: "Organizers and signers must hold a verified identity" },
      () => registry.connect(quick).createEvent("Quick water", "", [quick.address, s[19].address], 1, [U])
    )
  );
  step("2026-03-21T11:00:00Z", () =>
    attempt(
      { actor: dune.address, action: "List its own field wallet as an independent signer", rule: "Signers must be different organizations from the organizer" },
      () => registry.connect(dune).createEvent("Dune water points", "", [dune2.address, audit.address], 2, [U])
    )
  );

  // ------------------------------------------------ milestones helpers
  const propose = async (ev, by, slug, title, text, payee, token, amount) => {
    const d = doc(slug, title, text);
    await ev.connect(by).proposeMilestone(d.uri, payee, token, usd(amount));
  };
  const sign = (when, ev, id, signer, slug, title, content) =>
    step(when, async () => {
      const d = doc(slug, title, content);
      await ev().connect(signer).attest(id, d.hash, d.uri);
    });
  const E1 = () => e1;
  const E2 = () => e2;

  // ------------------------------------------- grant: parent -> child
  step("2026-04-20T09:00:00Z", () =>
    propose(e1, blue, "budget-grant-kwc", "Budget line: grant to Kayes Water Collective", "9,000 USDC to the event 'Wells for Village A & B'. Covers 2 boreholes (4,200 each) and spares (600).", e2a, U, 9000)
  );
  sign("2026-04-21T10:00:00Z", E1, 0, village, "att-grant-village", "Village councils: support letter", "Both village councils requested boreholes at the surveyed sites. We support this grant.");
  sign("2026-04-22T16:00:00Z", E1, 0, audit, "att-grant-audit", "Sahel Audit: partner due diligence", "Kayes Water Collective: registered since 2019, 2023-2025 accounts reviewed, no findings.");

  // ------------------------------------------------------- wells
  const survey = pic("photo-survey", "Siting survey map", "survey", "Siting survey · Kayes region · 2026-04-12", 3);
  step("2026-05-02T09:00:00Z", () =>
    propose(e2, kayes, "budget-well-a", "Budget line: borehole, Village A", "Drill a 60 m borehole and install a hand pump at Village A. Contractor: Sahel Drilling Co., quote SDC-2026-044 (transport included, line 7).", drilling.address, U, 4200)
  );
  sign("2026-05-03T10:00:00Z", E2, 0, village, "att-wella-village", "Village A council: site agreed", `Site agreed with the council, 120 m from the school. Survey: ${survey.uri}`);
  sign("2026-05-04T15:00:00Z", E2, 0, audit, "att-wella-audit", "Sahel Audit: quote check", "Quote SDC-2026-044: 4,200 USDC for 60 m with casing, pump and transport, within the 3,900 to 4,600 regional range.");
  step("2026-05-20T09:00:00Z", () =>
    propose(e2, kayes, "budget-well-b", "Budget line: borehole, Village B", "Drill a 55 m borehole and install a hand pump at Village B. Contractor: Sahel Drilling Co.", drilling.address, U, 4200)
  );
  sign("2026-05-21T10:00:00Z", E2, 1, village, "att-wellb-village", "Village B council: site agreed", "Site agreed, next to the market square.");
  sign("2026-05-22T15:00:00Z", E2, 1, audit, "att-wellb-audit", "Sahel Audit: quote check", "Quote SDC-2026-051: 4,200 USDC, within the regional range.");

  // ------------------------- trucks: argued in the open, never signed
  const trucksSubject = subjectOfMilestone(e1a, 1);
  step("2026-05-05T09:00:00Z", () =>
    propose(e1, blue, "budget-trucks", "Budget line: truck rental", "Truck rental to move drilling equipment, 2 trips: 2,500 EURC to Fast Freight Ltd.", truckco, E, 2500)
  );
  sign("2026-05-06T10:00:00Z", E1, 1, kayes, "att-trucks-kwc", "Kayes Water Collective: trucks needed", "Equipment must reach both villages before the rains.");
  step("2026-05-09T18:30:00Z", () => say(bob, trucksSubject, -1, "post-trucks-bob", "Bob", "Two trips on this route cost about 800 EUR at local rates. This quote is 3 times that."));
  step("2026-05-10T09:00:00Z", () => say(audit, trucksSubject, -1, "post-trucks-audit", "Sahel Audit Cooperative", "Transport is already in the drilling quote (SDC-2026-044, line 7). We will not sign this milestone."));
  step("2026-05-10T15:00:00Z", () => say(village, trucksSubject, -1, "post-trucks-village", "Village A & B councils", "Agreed with the auditor. We won't sign either."));
  step("2026-05-12T20:00:00Z", async () => {
    await vaults.connect(bob).setAllocation([e2a], [10000]);
    await say(bob, subjectOfAddress(blue.address), -1, "post-bob-redirect", "Bob", "Redirecting my monthly gift to the local collective directly until Blue Horizon explains the truck quote.");
  });
  step("2026-05-13T11:00:00Z", async () => {
    const d = doc("withdraw-trucks", "Blue Horizon: milestone withdrawn", "We withdraw the truck rental. The auditor is right: transport was in the drilling quote. We are reviewing how the quote reached us.");
    await e1.connect(blue).withdrawMilestone(1, d.uri);
    await forum.connect(blue).post(trucksSubject, ZERO, 0, d.uri, d.hash);
  });

  // ---------------------------------------------- curator reallocates
  step("2026-06-01T09:00:00Z", async () => {
    await vaults.connect(sahelWatch).setAllocation([e1a, e2a], [3000, 7000]);
    await say(sahelWatch, subjectOfAddress(sahelWatch.address), 0, "sw-alloc-2", "Sahel Watch: June allocation", "Moving to 30% / 70% in favour of Wells for Village A & B: Well A is drilled and signed off, and the truck quote episode shows the extra layer adds risk.");
  });
  step("2026-06-02T10:00:00Z", () => vaults.connect(crowd[5]).follow(sahelWatch.address));

  // -------------------------------------------------- deed reports
  const pA = [
    pic("photo-wella-drilling", "Well A: drilling", "drilling", "Village A · 2026-05-27 10:14 · 14.45012, -11.44031", 11),
    pic("photo-wella-pump", "Well A: pump installed", "pump", "Village A · 2026-06-10 16:02 · 14.45012, -11.44031", 12),
    pic("photo-wella-water", "Well A: first water", "water", "Village A · 2026-06-12 08:30 · 14.45012, -11.44031", 13),
  ];
  const repA = doc("report-well-a", "Completion report: borehole, Village A", [
    "Borehole completed on 2026-06-12, 62 m deep, static water level 31 m.",
    "Flow test: 1.4 m3/h over 6 hours. Hand pump: India Mark II.",
    "Serves about 640 people (council count), 120 m from the school.",
    "Paid: 4,200 USDC from 'Wells for Village A & B', milestone #0.",
    "Filed by: Sahel Drilling Co.",
  ].join("\n"));
  let deedA, deedB, deedDune, e3, e3a;
  step("2026-06-14T09:00:00Z", async () => {
    await impact.connect(drilling).reportDeed(e2a, 0, "water-well", 1445012, -1144031, false, repA.uri, repA.hash, pA.map((p) => p.hash), pA.map((p) => p.uri));
    deedA = await impact.deedIds(0);
  });
  const pB = [
    pic("photo-wellb-drilling", "Well B: drilling", "drilling", "Village B · 2026-06-20 09:51 · 14.46230, -11.41875", 21),
    pic("photo-wellb-pump", "Well B: pump installed", "pump", "Village B · 2026-07-02 15:20 · 14.46230, -11.41875", 22),
  ];
  const repB = doc("report-well-b", "Completion report: borehole, Village B", [
    "Borehole completed on 2026-07-02, 55 m deep, static water level 28 m.",
    "Flow test: 1.1 m3/h over 6 hours. Hand pump: India Mark II.",
    "Serves about 410 people, next to the market square.",
    "Paid: 4,200 USDC from 'Wells for Village A & B', milestone #1.",
    "Filed by: Sahel Drilling Co.",
  ].join("\n"));
  step("2026-07-08T09:00:00Z", async () => {
    await impact.connect(drilling).reportDeed(e2a, 1, "water-well", 1446230, -1141875, false, repB.uri, repB.hash, pB.map((p) => p.hash), pB.map((p) => p.uri));
    deedB = await impact.deedIds(1);
  });

  // ------------------------------------------------ on-site checks
  // `who` and `deed` may be functions: they only exist once earlier steps ran.
  const onDeed = (when, who, deed, stance, slug, title, text) =>
    step(when, () => say(typeof who === "function" ? who() : who, deed(), stance, slug, title, text));
  onDeed("2026-06-18T12:00:00Z", crowd[6], () => deedA, 1, "post-a-teacher", "Field note", "Pump works. Children fill bottles at break time.");
  onDeed("2026-06-20T17:00:00Z", audit, () => deedA, 1, "post-a-audit", "Sahel Audit: site visit", "Site visit on 2026-06-20. Depth log, pump model and flow match the report.");
  onDeed("2026-06-25T10:00:00Z", village, () => deedA, 1, "post-a-council", "Village A council", "The well is in use every day.");
  onDeed("2026-07-15T14:00:00Z", crowd[7], () => deedA, 1, "post-a-health", "Field note", "Visited on 2026-07-15, queue of about 15 people, water clear.");
  onDeed("2026-07-20T09:00:00Z", crowd[8], () => deedB, 1, "post-b-traveller", "Field note", "Saw the pump in use at the market square.");

  // ---------------------------------------------- your redirect (July)
  step("2026-07-01T19:00:00Z", async () => {
    await vaults.connect(you).follow(sahelWatch.address);
    await forum.connect(you).delegate(sahelWatch.address);
  });

  // ----------------------------- copycat claim: contested, argued out
  const fresh = pic("photo-dune-wella", "Dune Aid photo at Well A", "pump", "Village A · 2026-07-24 · 14.45019, -11.44037", 41);
  const griefers = [];
  step("2026-07-15T09:00:00Z", async () => {
    e3 = await created(registry.connect(dune).createEvent("Dune Aid water points", "", [village.address, audit.address], 2, [U]));
    e3a = await e3.getAddress();
    labels[e3a] = "Dune Aid water points (escrow)";
    for (let i = 0; i < 3; i++) {
      const g = await wallet(admin);
      labels[g.address] = `New wallet ${i + 1}`;
      griefers.push(g);
    }
  });
  step("2026-07-15T10:00:00Z", async () => {
    await fund(crowd[10], usdc, 500, e3);
    await propose(e3, dune, "budget-dune", "Budget line: pump rehabilitation, Village C", "Replace the rising main and cylinder of the hand pump at Village C.", duneBuilders, U, 500);
  });
  sign("2026-07-16T10:00:00Z", () => e3, 0, village, "att-dune-village", "Village councils: Village C pump", "Village C's council confirms the pump needs rehabilitation.");
  sign("2026-07-17T10:00:00Z", () => e3, 0, audit, "att-dune-audit", "Sahel Audit: quote check", "Quote for Village C: 500 USDC, in line with parts prices.");
  const repDune = doc("report-dune", "Completion report: water point, Village A", "Dune Aid rehabilitated and equipped the borehole at Village A in July 2026.\nFiled by: Dune Aid.");
  step("2026-07-25T09:00:00Z", async () => {
    // Same spot (8 m from Well A) and one of Well A's photos: flagged, not refused.
    await impact.connect(dune).reportDeed(e3a, 0, "water-well", 1445019, -1144037, false, repDune.uri, repDune.hash, [fresh.hash, pA[1].hash], [fresh.uri, pA[1].uri]);
    deedDune = await impact.deedIds(2);
  });
  onDeed("2026-07-25T12:00:00Z", dune2, () => deedDune, 1, "post-dune-self", "Dune Aid", "Our team did this work. Photos attached to the report.");
  onDeed("2026-07-25T13:00:00Z", dune, () => deedA, -1, "post-dune-on-a", "Dune Aid", "Well A was built in partnership with Dune Aid.");
  onDeed("2026-07-26T09:00:00Z", kayes, () => deedDune, -1, "post-dune-kwc", "Kayes Water Collective", "Well A was drilled in May by Sahel Drilling under our milestone, reported on 14 June with the depth log. One of the photos in this report is ours.");
  onDeed("2026-07-26T10:00:00Z", () => griefers[0], () => deedDune, 1, "post-dune-g1", "Field note", "I saw Dune Aid there.");
  onDeed("2026-07-26T10:05:00Z", () => griefers[1], () => deedDune, 1, "post-dune-g2", "Field note", "Dune Aid did great work.");
  onDeed("2026-07-26T10:09:00Z", () => griefers[2], () => deedDune, 1, "post-dune-g3", "Field note", "Confirmed.");
  onDeed("2026-07-27T11:00:00Z", audit, () => deedDune, -1, "post-dune-audit", "Sahel Audit Cooperative", "We signed Dune Aid's milestone for a pump in Village C, not Village A. There is one borehole at this spot and we audited it in June. Dune Aid has not shown us any work in Village C.");
  onDeed("2026-07-27T15:00:00Z", village, () => deedDune, -1, "post-dune-village", "Village A & B councils", "No Dune Aid team has worked in Village A.");
  onDeed("2026-07-28T09:00:00Z", sahelWatch, () => deedDune, -1, "post-dune-sw", "Sahel Watch", "The evidence points one way. We will not fund Dune Aid, and we flag their Village C milestone for a site visit.");
  step("2026-07-28T10:00:00Z", () => say(sahelWatch, subjectOfAddress(dune.address), -1, "post-sw-dune-org", "Sahel Watch", "Dune Aid claimed another organization's well. Not recommended."));

  // --------------------------------- Bob's EURC at work: mechanics
  step("2026-08-10T09:00:00Z", () =>
    propose(e2, kayes, "budget-mechanics", "Budget line: hand-pump mechanic training", "Train 4 local mechanics (2 per village) and give them tool kits: 800 EURC to the Kayes Pump Mechanics Guild.", guild, E, 800)
  );
  sign("2026-08-11T10:00:00Z", E2, 2, village, "att-mech-village", "Village councils: trainees named", "Two trainees per village chosen at the council meeting.");
  sign("2026-08-12T10:00:00Z", E2, 2, audit, "att-mech-audit", "Sahel Audit: guild check", "Guild registered, training plan and tool list checked.");

  // ---------------------------------------------- water testing (E1)
  step("2026-09-01T09:00:00Z", () =>
    propose(e1, blue, "budget-lab", "Budget line: water quality tests", "Lab tests for both boreholes (bacteria, nitrates, arsenic, fluoride): 400 USDC to Bamako Water Lab.", lab, U, 400)
  );
  sign("2026-09-02T10:00:00Z", E1, 2, kayes, "att-lab-kwc", "Kayes Water Collective: samples sent", "Samples from both wells sent on 2026-08-28.");
  sign("2026-09-03T10:00:00Z", E1, 2, audit, "att-lab-audit", "Sahel Audit: lab results", "Both wells pass WHO guidelines. E. coli: 0 CFU/100 mL.");

  // ------------------------------ shelter: sensitive, location protected
  step("2026-09-20T09:00:00Z", () =>
    propose(e1, blue, "budget-shelter", "Budget line: water tank for a women's shelter", "5,000 L tank and gutters for a shelter run by a local partner. Partner and location withheld for safety.", shelter, U, 300)
  );
  sign("2026-09-21T10:00:00Z", E1, 3, kayes, "att-shelter-kwc", "Kayes Water Collective: partner known to us", "We have worked with this partner since 2022.");
  sign("2026-09-22T10:00:00Z", E1, 3, audit, "att-shelter-audit", "Sahel Audit: checked privately", "Invoice and site checked by our staff. Details held privately.");
  const pS = [pic("photo-shelter-tank", "Shelter: tank installed", "water", "Location withheld · 2026-09-30", 51)];
  const repS = doc("report-shelter", "Completion report: water tank, protected site", "5,000 L tank and gutters installed on 2026-09-30.\nLocation published at ~11 km precision only. Partner name withheld.\nFiled by: Blue Horizon Foundation.");
  step("2026-10-02T09:00:00Z", () =>
    impact.connect(blue).reportDeed(e1a, 3, "water-tank", 1452345, -1139876, true, repS.uri, repS.hash, pS.map((p) => p.hash), pS.map((p) => p.uri))
  );

  // ------------------------------------- Well B breaks, then repaired
  const broken = pic("photo-wellb-broken", "Well B: pump handle broken", "broken", "Village B · 2026-10-11 07:55 · 14.46230, -11.41875", 31);
  const resident = await wallet(admin);
  labels[resident.address] = "Village B resident";
  onDeed("2026-10-12T08:00:00Z", resident, () => deedB, -1, "post-b-resident", "Dispute: pump broken", `Pump handle broke on 2026-10-09; nobody can draw water. Photo: ${broken.uri}`);
  onDeed("2026-10-12T12:00:00Z", village, () => deedB, -1, "post-b-council-broken", "Village B council", "Confirmed: the handle is broken. Repair requested from Kayes Water Collective.");
  step("2026-10-20T09:00:00Z", () =>
    propose(e2, kayes, "budget-spares", "Budget line: spare parts and repair", "Spare parts kit for both pumps (handles, seals, rods) and repair of the Village B handle: 600 USDC to HandPump Works.", pumpworks, U, 600)
  );
  sign("2026-10-21T10:00:00Z", E2, 3, village, "att-spares-village", "Village B council: repair needed", "The Village B pump handle broke on 2026-10-09.");
  sign("2026-10-23T10:00:00Z", E2, 3, audit, "att-spares-audit", "Sahel Audit: quote check", "Quote HPW-311: 600 USDC, in line with catalogue prices.");
  const fixed = pic("photo-wellb-fixed", "Well B: repaired", "water", "Village B · 2026-10-25 09:10 · 14.46230, -11.41875", 32);
  onDeed("2026-10-25T12:00:00Z", resident, () => deedB, 1, "post-b-resident-fixed", "Field note", `Repaired by a guild mechanic this morning. Water flowing again. Photo: ${fixed.uri}`);
  onDeed("2026-10-26T09:00:00Z", village, () => deedB, 1, "post-b-council-fixed", "Village B council", "Repair done on 2026-10-25. The pump works.");

  // ----------------------------------------------- pending at "now"
  step("2026-10-25T15:00:00Z", () =>
    propose(e2, kayes, "budget-latrines", "Budget line: school latrines, Village A", "Four latrine blocks with handwashing points at the Village A school: 1,500 USDC to local builders (quotes attached).", builder, U, 1500)
  );
  sign("2026-10-26T10:00:00Z", E2, 4, village, "att-latrines-village", "Village A council: school request", "The school committee asked for these latrines.");
  step("2026-10-27T18:00:00Z", () => network.provider.send("evm_mine"));

  for (const p of plan.sort((a, b) => a.t - b.t || a.n - b.n)) {
    await at(p.t);
    await p.fn();
  }

  // ------------------------------------------------------- output
  await printTrail(e1, labels);
  await printTrail(e2, labels);

  const snapshot = await buildSnapshot({
    identity,
    registry,
    impact,
    vaults,
    forum,
    labels,
    docs,
    blocked,
    meta: { network: "Local demo chain (Hardhat)", generatedBy: "scripts/demo.js", you: you.address },
  });
  const out = path.join(__dirname, "..", "site", "data.js");
  fs.writeFileSync(out, `// Generated by scripts/demo.js. Do not edit.\nwindow.TRAIL = ${JSON.stringify(snapshot)};\n`);
  console.log(`\nwrote ${path.relative(process.cwd(), out)} (${(fs.statSync(out).size / 1024).toFixed(0)} KiB), ${blocked.length} blocked attempts`);
}

main().catch((err) => {
  console.error(err);
  process.exitCode = 1;
});
