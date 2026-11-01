# charity-trail

A proof of concept for traceable charity money, on an Ethereum L2 (Base;
testnet Base Sepolia).

- Each charity *event* is an escrow contract holding stablecoins.
- Money leaves only through milestones signed off by independent, verified
  organizations.
- Events can fund other events, and money is traced hop by hop down to a
  public report of the work ("a well in Village A").
- **Nobody can freeze money.** The check on bad behaviour is that everything
  is public and donors' *next* gifts can go elsewhere: monthly giving,
  one-transaction redirects, curators to follow, and open argument weighted
  by money actually given.

## Flow

```
donor ──deposit──▶ GivingVaults ──monthly, by own split or a curator's──▶ CharityEvent (escrow)
                     ▲  redirect / follow / withdraw: 1 tx                       │
                     │                                       organizer proposes milestone
                     │                                                           ▼
                     │                                N-of-M verified signers sign (evidence hash)
                     │                                                           ▼
                     │                                 paid to payee, or into a child event
                     │                                                           ▼
                     │                           ImpactRegistry: report with photos by sha256;
                     │                           overlapping claims flagged "contested"
                     │                                                           ▼
                     └──── donors & curators read ◀── Forum: posts with stance + evidence,
                                                     weighted off-chain by money given
```

## Contracts

| File | Role |
|---|---|
| `contracts/CharityEvent.sol` | Escrow, milestones, sign-offs (pays at threshold), withdraw, close + refunds, child events |
| `contracts/EventRegistry.sol` | Creates events after checking signer identity and independence; public index |
| `contracts/IdentityRegistry.sol` | Trusted issuers vouch "wallet W belongs to org O" |
| `contracts/GivingVaults.sol` | Monthly giving: balance, monthly amount, own split or follow a curator, withdraw |
| `contracts/ImpactRegistry.sol` | Deed reports; overlaps (same spot, reused photo) flagged as contested; sensitive mode |
| `contracts/Forum.sol` | Posts on any deed/milestone/org with a stance and evidence hash; delegation |
| `contracts/MockStablecoin.sol` | 6-decimal test token for local runs |

## Rules enforced on-chain

- **Verified identities:** organizers and signers need a credential from a
  trusted issuer. Credentials can be revoked.
- **Independent signers:** signers must be different organizations from the
  organizer and from each other. Several wallets of one org count as one.
- **Conflicts of interest:**
  - the organizer can't be a signer
  - a payee can't sign off their own payment
  - the organizer of a child event can't sign a grant into it
- **No over-commitment:** a milestone can only earmark money that has been
  donated and not yet committed.
- **Gates, not freezes:** a milestone pays out with the signature that
  reaches the threshold. If signers refuse, the organizer can withdraw it.
- **One report per paid milestone:** a report needs a paid milestone.
- **Overlaps are flagged, not blocked:** a report in the same ~11 m cell (or
  its 8 neighbours) as another of the same category, or reusing a photo hash,
  marks both as contested. A hard lock would let a fast copycat block the
  real builder.
- **Sensitive reports:** stored at 0.1° (~11 km) precision and never matched
  by place.
- **Monthly giving:**
  - at most one payment per 28 days, triggered by anyone (a keeper)
  - a split can only target registered events
  - closed events are skipped, and their share stays in the vault

## Off-chain, reproducible from logs

- **Money tracing (`site/flow.js`):** each pool is replayed in time order. A
  payment is drawn pro rata from every donor's *unspent* money at that
  moment, so later gifts never get credit for earlier payments. Grants into
  child events are followed the same way.
- **Discussion weight:** an author's weight is what they gave that was paid
  out through signed milestones. Each author's latest stance counts.
  Delegators who didn't post themselves add their weight to their delegate.
  The site shows three separate lanes:
  - weighted by money given
  - verified organizations, excluding the claimant's own org
  - other wallets, shown but not counted

## Run it

```sh
npm install
npm test          # 26 tests
npm run demo      # 8-month story on a local chain, writes site/data.js
npm run site      # same, then serves the tracker at http://localhost:8000
```

The demo story uses fictional organizations and generated photos:

- **Donors and curators:**
  - a crowd of one-off donors
  - you (20 USDC a month)
  - Bob (EURC)
  - two curators: Sahel Watch and GiveWise
- **Overpriced truck quote:** argued down in public. Signers refuse, the
  organizer withdraws it, and Bob redirects his monthly gift to the local
  collective.
- **Curator shift:** Sahel Watch moves its split toward the local
  organization, and you start following it.
- **Copycat claim:** Dune Aid claims Well A as its own work. The claim is
  flagged and argued out: it has zero weighted support, and the signers and
  village dispute it.
- **Broken pump:** Well B's pump breaks. It's disputed, repaired with signed
  spare parts, and confirmed again.
- **Protected location:** a shelter's water tank is reported at coarse
  precision, with the partner's name withheld.

## Tracker site (`site/`)

- **Your money:** what you gave and where it went, your monthly split and its
  history, how to redirect, the flow diagram, and every hop down to the
  reports.
- **Impact:** photos and reports re-hashed live in the browser, the map
  overlap zone, contested claims, who paid, and the weighted public checks.
- **Events:** escrow balances, new money per month by source (one-off,
  monthly, grants), milestones with their discussions.
- **Curators:** current split, history, followers, and followers' money per
  month.
- **How it works:**
  - the levers
  - protections for field workers
  - the hard rules, with refused attempts
  - verified orgs
  - known limits

## Deploy to Base Sepolia

```sh
export PRIVATE_KEY=0x...            # throwaway testnet key
npm run deploy                      # all contracts; deployer becomes an issuer
EVENT=0x... npm run trail           # print an event's trail
```

Get test ETH from a Base Sepolia faucet and test USDC/EURC from
<https://faucet.circle.com>. Stablecoin addresses are in `scripts/deploy.js`.
Check them against Circle's docs first.

## Known limits

- **Money already given stays given.** Only the organizer closing the event
  returns unspent money. Exit protects future money only.
- **Weight from giving favours big donors.** Square-root weighting would
  soften it but needs proof of unique personhood. Self-dealing (giving to
  your own event and paying yourself) still needs complicit independent
  signers.
- **Signing rings** (orgs signing for each other) are visible, not prevented.
- **Hashes prove a file is unchanged, not that it's true.** Paid, independent
  site visits are the real check.
- **Admins:** the identity registry admin and issuers are single addresses.
  They should become multisigs, or be chosen by donor-weighted votes.
- **Permanence:** the record can't be deleted. Never post anything that
  identifies beneficiaries or could endanger field staff later.
- **Other gaps:** no anonymous-but-verified posting yet (zero-knowledge
  membership proofs), no audit, no IPFS pinning. Don't hold real money with
  this.
- **Legal:** donor control over spending can affect tax deductibility in
  some countries.
