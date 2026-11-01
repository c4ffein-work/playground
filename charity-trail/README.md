# charity-trail

A proof of concept for traceable charity money. Each charity *event*, such as
"Flood relief, Region X", is a contract holding donations in stablecoins.
Money only leaves for a stated purpose (a **milestone**), and only after
independent stakeholders have publicly signed off with evidence.

Target: an Ethereum L2 (Base). Testnet: Base Sepolia.

## Flow

```
donors ──USDC/EURC──▶ CharityEvent (escrow)
                         │
 organizer proposes      ▼
 milestone ───────▶ [Proposed] ── N-of-M attesters sign with evidence hash/URI
                         ▼
                    [Approved] ── challenge window (e.g. 3 days)
                     │      │
     undisputed ─────┘      └──── donor/attester challenges ──▶ [Disputed]
         ▼                                                        │
    [Released] ◀──── arbiter upholds ─────────────────────────────┤
     payee paid                                                   ▼
                                                   arbiter rejects ─▶ [Rejected]
                                                   (funds back in the pool)

arbiter can cancel the event ──▶ donors claim unreleased funds back, pro rata
```

## Rules enforced on-chain

- **Several stablecoins:** whitelisted per event, each with its own books.
- **No over-commitment:** a milestone can only earmark money that has been
  donated and not yet committed.
- **Conflicts of interest:**
  - the organizer can't be an attester
  - a payee can't attest their own milestone
- **Every attestation carries evidence:** a hash pinning the document, plus a
  URI pointing to it (IPFS, for example).
- **Challenges:** any donor (in that token) or attester can challenge during
  the window, which freezes the payout.
- **Everything is an event:** `scripts/trail.js` rebuilds the full public
  trail from logs only.

## Contracts

| File | Role |
|---|---|
| `contracts/CharityEvent.sol` | Escrow, milestones, attestations, disputes, refunds |
| `contracts/EventRegistry.sol` | Deploys events and indexes them, so one address lists them all |
| `contracts/MockStablecoin.sol` | 6-decimal test token for local runs |

## Run it

```sh
npm install
npm test          # 15 tests
npm run demo      # full story on a local chain, prints the trail
```

Demo output (abridged):

```
donation   alice gave 5000.0 USDC
donation   bob gave 3000.0 EURC
milestone  #0 proposed: 3200.0 USDC -> pumpCo  [ipfs://budget/water-pumps]
attest     #0 signed by localNgo  [ipfs://evidence/pump-invoice]
attest     #0 signed by villageRep  [ipfs://evidence/pumps-photos]
PAID       #0 3200.0 USDC -> pumpCo
milestone  #1 proposed: 2500.0 EURC -> truckCo  [ipfs://budget/truck-rental]
...
CHALLENGE  #1 by bob  [ipfs://challenge/quote-is-3x-market-price]
ruling     #1 REJECTED  [ipfs://ruling/overpriced]
```

## Deploy to Base Sepolia

```sh
export PRIVATE_KEY=0x...            # throwaway testnet key
npm run deploy                      # registry only
ARBITER=0x... ATTESTERS=0xa,0xb,0xc THRESHOLD=2 npm run deploy   # + one event
EVENT=0x... npm run trail           # print an event's trail
```

Get test ETH from a Base Sepolia faucet and test USDC/EURC from
<https://faucet.circle.com>. Stablecoin addresses are in `scripts/deploy.js`.
Check them against Circle's docs first.

Moving to mainnet or another L2 means adding a network to
`hardhat.config.js` and the stablecoin addresses to `scripts/deploy.js`.

## Not done yet (PoC limits)

- **Arbiter:** a single address. Should become a Safe multisig, or a
  staked/elected panel.
- **Attesters:** fixed per event, with no stake or reputation. Next step:
  mirror attestations to [EAS](https://attest.org) so reputation carries over
  between events.
- **Privacy:** none. Beneficiaries must never be identifiable on-chain. Use
  aggregate payees (local partner wallets) and zero-knowledge proofs for
  individual aid.
- **Off-ramp:** the trail ends when the payee converts to fiat.
- **Audit:** none. Don't hold real money with this.
- **Front-end:** none. The trail script is the "dashboard" for now.
