import { describe, expect, test } from "bun:test";
import { decodeEntities, frDateToIso, parseCityCinemaNames, parseShowings } from "../src/ugc";

const fixture = await Bun.file(new URL("./fixtures/showings.html", import.meta.url)).text();

describe("parseShowings", () => {
  const { films, showings } = parseShowings(fixture);

  test("finds every film block and every screening card", () => {
    expect(films).toHaveLength(2);
    expect(films[0].key).toBe("delivre-nous-du-mal");
    expect(films[1].key).toMatch(/^[a-z0-9-]+$/);
    expect(showings).toHaveLength(5);
  });

  test("film metadata is decoded and trimmed", () => {
    const doc = films[0];
    expect(doc.title).toBe("DELIVRE-NOUS DU MAL");
    expect(doc.genre).toBe("Documentaire");
    expect(doc.duration).toBe("1h49");
    expect(doc.release).toBe("30 septembre 2026");
    expect(doc.director).toBe("Valeria Baldan, Giovanni Ziberna");
    expect(doc.synopsis).toContain("Association Internationale des Exorcistes");
    expect(doc.label).toBe("Sélection UGC Docs");
    expect(doc.rating).toBe(3.2);
    expect(doc.poster).toMatch(/^https:\/\/www\.ugc\.fr\/dynamique\/films\/.*\.jpg$/);
    expect(films[1].genre).toBe("Action, Comédie");
    expect(films[1].duration).toBe("2h09");
  });

  test("screening cards carry the planning fields", () => {
    const s = showings[0];
    expect(s).toMatchObject({
      id: "ugc:330401306442", filmKey: "delivre-nous-du-mal", cinemaId: "ugc:32", date: "2026-10-04", time: "18:30",
      endTime: "20:35", version: "VF", extra: "", room: "Salle 1",
      bookingUrl: "https://www.ugc.fr/reservationSeances.html?id=330401306442",
    });
    expect(showings.map((x) => x.time)).toEqual(["18:30", "11:00", "14:00", "17:00", "20:00"]);
    expect(showings.map((x) => x.version)).toEqual(["VF", "VOST", "VOST", "VF", "VOST"]);
    expect(new Set(showings.slice(1).map((x) => x.filmKey))).toEqual(new Set([films[1].key]));
  });

  test("an empty page yields nothing rather than throwing", () => {
    expect(parseShowings("<div>Aucune séance</div>")).toEqual({ films: [], showings: [] });
  });
});

test("decodeEntities handles named, decimal and hex entities", () => {
  expect(decodeEntities("Cin&eacute; Cit&eacute; &amp; l&rsquo;Op&#233;ra &#x153;uvre")).toBe("Ciné Cité & l’Opéra œuvre");
  expect(decodeEntities("&unknown;")).toBe("&unknown;");
});

test("frDateToIso", () => {
  expect(frDateToIso("04/10/2026")).toBe("2026-10-04");
  expect(frDateToIso("2026-10-04")).toBe("2026-10-04");
});

test("parseCityCinemaNames reads the zoom anchors of a city tab", () => {
  const html = `
    <a href="cinema-ugc-talence.html" class="zoom" title="UGC Talence"><img/></a>
    <a href="cinema-ugc-cine-cite-bordeaux.html" class="zoom" title="UGC Cin&eacute; Cit&eacute; Bordeaux Gambetta"></a>
    <a href="cinema-ugc-talence.html" class="zoom" title="UGC Talence"></a>`;
  expect(parseCityCinemaNames(html)).toEqual(["UGC Talence", "UGC Ciné Cité Bordeaux Gambetta"]);
});
