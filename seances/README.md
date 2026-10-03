# Séances

Private movie-night planner for a group of friends: the UGC **and CGR**
programme of your city, and everyone ticks the séances they could make. The
séance most people can attend floats to the top. Bun + SQLite, **zero
dependencies**, one process.

```sh
cd seances
bun start            # http://localhost:3000, database in ./seances.db
PORT=8080 SEANCES_DB=/var/lib/seances/db.sqlite bun start
bun run scrape       # one-off programme refresh from the CLI (same DB)
bun test
```

First visit: pick your city (Bordeaux, Lyon, Paris, …). The app resolves the
UGC and CGR cinemas there (Bordeaux: 3 UGC + CGR Le Français + CGR Villenave
d'Ornon) and scrapes their programme for the next 8 days. Untick a cinema chip
to leave it out. It re-scrapes every 6 hours and on the **refresh** button.

## How it works

- **No browser scraping.** ugc.fr serves its screenings through a plain GET
  (`showingsCinemaAjaxAction!getShowingsForCinemaPage.action?cinemaId=…&date=…`)
  returning an HTML fragment whose screening cards carry data-attributes
  (`data-showing`, `data-filmId`, `data-seanceHour`, `data-version`, …).
  `src/ugc.ts` parses those with regexes. Cities come from the site's city tabs
  (`cinemasAjaxAction!getCinemasList.action?id=<region>`), cinema ids from the
  newsletter JSON (`inscriptionNewsletterAction!getCinemaList.action`).
  One request per cinema per day, sequential, 400 ms apart.
- **No browser scraping for CGR either.** cgrcinemas.fr is a Webedia React
  site whose showtimes widget calls same-origin JSON routes
  (`/api/gatsby-source-boxofficeapi/schedule?from=…&to=…&theaters={"id":"W3300","timeZone":"Europe/Paris"}`
  for a whole date range, `…/movies?ids=…` for metadata, ≤ 50 per call).
  `src/cgr.ts` maps its tags to VF / VO / VOST / VFST and ICE / 3D / IV
  extras. Theaters come from the home page's theater selector (base64
  `Theater:W3300` ids) plus the showtimes index page-data, whose slugs embed
  the metro city for suburbs (`villenave-dornon-bordeaux`) — that is how a
  city is matched to its CGRs. If one chain's lookup fails, the city still
  loads with the other.
- **One card per film across chains.** Films are keyed by their normalized
  title (`filmKey`), so "CARS" at UGC and "Cars" at CGR share a card, votes
  and séance list. Metadata merges field by field (longest synopsis, any
  rating, nicer-cased title).
- **Identity is a cookie.** The first visit mints an id; you give it a name.
  Linking an email or phone (optional, never contacted) lets you *take over*
  that member from another device by typing the same contact. It is
  trust-based: the app is meant to live behind a private URL for people who
  trust each other, there is no verification.
- **Votes.** Per séance: "I can make it" (availability). Per film: 👍 / 👎.
  **Best plans** = séances ranked by people available, then the film's net
  thumbs, then the sooner one. Every séance links to its chain's booking page.
- **Data model** (`src/db.ts`): `cinemas` (`ugc:32`, `cgr:W3300`), `films`
  (by title key), `showings` (namespaced ids), `users`, `availability`,
  `film_votes`, `scrapes`, `settings`. A re-scrape replaces a cinema's
  showings on the scraped days and cascades votes of séances that
  disappeared. Changing city wipes votes. A `schema_version` bump resets an
  older database file.

## API (all JSON, cookie-identified)

| Method | Path | Body |
| --- | --- | --- |
| GET | `/api/state` | — everything the page shows |
| POST | `/api/me` | `{ name, email?, phone? }` |
| POST | `/api/login` | `{ contact }` adopt the member linked to that email / phone |
| POST | `/api/city` | `{ regionId }` (ids in `/api/state`'s `cities`) |
| POST | `/api/cinema` | `{ id, selected }` include / hide a cinema (`"ugc:1"`, `"cgr:W3300"`) |
| POST | `/api/availability` | `{ showingId, on }` |
| POST | `/api/film-vote` | `{ filmKey, vote: -1 \| 0 \| 1 }` |
| POST | `/api/refresh` | `{ days? }` re-scrape in the background |

## Self-hosting notes

- Put it behind HTTPS with an unguessable path or basic auth at the reverse
  proxy; the app itself has no login wall.
- Back up `seances.db` (WAL mode: copy `.db`, `.db-wal`, `.db-shm` together
  or use `sqlite3 seances.db ".backup out.db"`).
- The scraper fails loudly when UGC's markup stops matching
  (`unrecognized showings page`), and the page shows the last scrape error.
  `test/fixtures/showings.html` is a real UGC fragment and
  `test/fixtures/cgr-*.json` real CGR payloads, all from 2026-10-03; if a
  chain changes its markup or API, refresh the fixture and its parser.
