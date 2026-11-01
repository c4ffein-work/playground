# charity-trail

A proof of concept for traceable charity money. Each charity *event*, such as
"Clean water for Kayes 2026", is a contract holding donations in stablecoins.
Money only leaves for a stated purpose (a **milestone**), and only after
independent, verified organizations have publicly signed off with evidence.
Events can fund other events, and the real-world result (a well, say) gets a
public report that nobody else can claim.

Target: an Ethereum L2 (Base). Testnet: Base Sepolia.

## Flow

```
donors ──USDC/EURC──▶ CharityEvent (escrow)
                         │
 organizer proposes      ▼
 milestone ───────▶ [Proposed] ── N-of-M verified signers sign with evidence hash/URI
                         ▼
                    [Approved] ── challenge window (e.g. 7 days)
                     │      │
     undisputed ─────┘      └──── donor/signer challenges ──▶ [Disputed]
         ▼                                                       │
    [Released] ◀──── arbiter upholds ────────────────────────────┤
     payee paid, or                                              ▼
     child event funded                          arbiter rejects ─▶ [Rejected]
         │                                       (funds back in the pool)
         ▼
 ImpactRegistry: payee/organizer files a deed report (photos + report by sha256,
 GPS spot locked) ──▶ anyone confirms or disputes it on site

arbiter can cancel the event ──▶ donors claim unreleased funds back, pro rata
```

## Rules enforced on-chain

- **Verified identities:** organizers, signers and arbiters need a credential
  from a trusted issuer (a charity register, auditor or vLEI issuer) in
  `IdentityRegistry`. Credentials can be revoked.
- **Independent signers:** each signer must be a different organization from
  the organizer and from the other signers. Several wallets of one org count
  as one.
- **Conflicts of interest:**
  - the organizer can't be a signer
  - a payee can't sign off their own payment
  - the organizer of a child event can't sign a grant into it
- **No over-commitment:** a milestone can only earmark money that has been
  donated and not yet committed.
- **Several stablecoins:** whitelisted per event, each with its own books.
- **Every sign-off carries evidence:** a hash pinning the document, plus a
  URI pointing to it.
- **Challenges:** any donor (in that token) or signer can challenge during
  the window, which freezes the payout until the arbiter rules.
- **Multi-org flows:** a milestone can pay into another event, e.g. a global
  NGO funding a local NGO's event. The money lands there as a donation, so it
  can be followed hop by hop. If that child event is cancelled, the parent
  pulls its share back.
- **Impact reports:** a report of real-world work (a "deed") needs a paid
  milestone and pins its report and photos by sha256. The same work can't be
  claimed twice:
  - the deed locks its ~11 m map cell and the 8 neighbouring cells for its
    category
  - each photo hash can back one deed only
  - each milestone can back one deed only
- **Public checks:** anyone can confirm or dispute a deed once. Votes from
  verified orgs are counted separately.

## Contracts

| File | Role |
|---|---|
| `contracts/CharityEvent.sol` | Escrow, milestones, sign-offs, disputes, refunds, child events |
| `contracts/EventRegistry.sol` | Creates events after checking signer identity and independence; public index |
| `contracts/IdentityRegistry.sol` | Trusted issuers vouch "wallet W belongs to org O" |
| `contracts/ImpactRegistry.sol` | Deed reports, duplicate-claim locks, public confirmations |
| `contracts/MockStablecoin.sol` | 6-decimal test token for local runs |

## Run it

```sh
npm install
npm test          # 26 tests
npm run demo      # 8-month story on a local chain, writes site/data.js
npm run site      # same, then serves the tracker at http://localhost:8000
```

## Tracker site (`site/`)

A static page that follows a donation to the work it paid for:

- **Your money:**
  - pick a donor and see their share split into: built and reported, paid
    for services, signed but waiting, and not spent yet
  - follow the path as a flow diagram and step by step, through every
    organization, down to the well
- **Impact:** each deed shows:
  - its photos and report, with sha256 re-checked live in the browser against
    the on-chain hash
  - the locked map cells
  - who paid for it
  - on-site confirmations and disputes
  - refused duplicate claims
- **Events:** escrow balances, milestones, sign-offs, challenges and rulings.
- **Safeguards:** every refused attempt to break the rules, and the list of
  verified organizations.

`scripts/lib/snapshot.js` rebuilds everything from chain logs. The site only
reads that snapshot plus the off-chain files, so it could point at any
network. The demo uses fictional organizations, and its photos are generated
illustrations.

Money tracing is pro rata (`site/flow.js`): your part of each payment is what
you gave, divided by the pool, times the payment. This repeats at every hop.

## Deploy to Base Sepolia

```sh
export PRIVATE_KEY=0x...            # throwaway testnet key
npm run deploy                      # 3 registries; deployer becomes an issuer
EVENT=0x... npm run trail           # print an event's trail
```

Get test ETH from a Base Sepolia faucet and test USDC/EURC from
<https://faucet.circle.com>. Stablecoin addresses are in `scripts/deploy.js`.
Check them against Circle's docs first.

## Not done yet (PoC limits)

- **Arbiter and registry admin:** single addresses. Should become Safe
  multisigs, or a staked/elected panel.
- **Signers:** fixed per event, with no stake or reputation. Next step:
  mirror sign-offs to [EAS](https://attest.org) so reputation carries over
  between events.
- **Deed locks are heuristics:**
  - a different well 15 m away is refused
  - re-encoding a photo changes its hash
  - production needs perceptual photo hashes and human review
- **Off-chain files:** the demo inlines them. Real use needs IPFS pinning or
  similar.
- **Privacy:** beneficiaries must never be identifiable on-chain. Use
  aggregate payees (local partner wallets) and zero-knowledge proofs for
  individual aid.
- **Off-ramp:** the trail ends when the payee converts to fiat.
- **Audit:** none. Don't hold real money with this.
