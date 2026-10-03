import { describe, expect, test } from "bun:test";
import { movieToFilm, parseCgrSchedule, parseTheaterOptions, parseTheaterSlugs, theatersInCity } from "../src/cgr";
import { addSeconds, filmKey, hm } from "../src/types";

const schedule = await Bun.file(new URL("./fixtures/cgr-schedule.json", import.meta.url)).json();
const movies = await Bun.file(new URL("./fixtures/cgr-movies.json", import.meta.url)).json();
const options = await Bun.file(new URL("./fixtures/cgr-theater-options.html", import.meta.url)).text();
const horaire = await Bun.file(new URL("./fixtures/cgr-horaire-film.json", import.meta.url)).json();

describe("parseCgrSchedule", () => {
  const { films, showings } = parseCgrSchedule(schedule, movies, "W3300");

  test("one film per movie, one showing per schedule entry", () => {
    expect(films.map((f) => f.title).sort()).toEqual(["Cars", "Digger", "La Pat' Patrouille : Le film mission Dino"]);
    expect(showings).toHaveLength(5);
  });

  test("movie metadata maps onto the shared Film shape", () => {
    const cars = films.find((f) => f.title === "Cars")!;
    expect(cars).toMatchObject({ key: "cars", genre: "Animation, Comédie, Action, Fantastique", duration: "1h57", director: "John Lasseter", release: "14/06/2006", label: "", rating: null });
    expect(cars.poster).toMatch(/^https:\/\/.*\.jpg$/);
    expect(cars.synopsis).toContain("Flash McQueen");
  });

  test("showings: ids, versions, extras, rooms, booking links, end times", () => {
    const ice = showings.find((s) => s.id === "cgr:W3300:55774:2026-10-04:10:30")!;
    expect(ice).toMatchObject({ filmKey: "cars", cinemaId: "cgr:W3300", date: "2026-10-04", time: "10:30", endTime: "12:27", version: "VF", extra: "ICE", room: "Salle ICE" });
    expect(ice.bookingUrl).toBe("https://achat.cgrcinemas.fr/lefrancais/r/435133");
    const plain = showings.find((s) => s.time === "15:40")!;
    expect(plain).toMatchObject({ version: "VF", extra: "", room: "Salle 11" });
    const vost = showings.find((s) => s.filmKey === "digger")!;
    expect(vost).toMatchObject({ version: "VOST", extra: "ICE", time: "19:30", endTime: "21:39" });
  });

  test("a movie whose metadata is missing is skipped, not crashed on", () => {
    const { films, showings } = parseCgrSchedule(schedule, movies.filter((m: any) => m.id !== "55774"), "W3300");
    expect(films.map((f) => f.key)).not.toContain("cars");
    expect(showings.every((s) => s.filmKey !== "cars")).toBe(true);
  });

  test("a payload without the theater throws", () => {
    expect(() => parseCgrSchedule({}, [], "W3300")).toThrow(/no schedule/);
  });
});

test("movieToFilm tolerates sparse records", () => {
  expect(movieToFilm({ id: "1", title: "X" })).toMatchObject({ key: "x", title: "X", duration: "", release: "", director: "", synopsis: "", poster: "" });
});

describe("theaters", () => {
  const names = parseTheaterOptions(options);
  const slugs = parseTheaterSlugs(horaire);

  test("the home page selector gives every theater a decoded id and a clean name", () => {
    expect(names.size).toBe(73);
    expect(names.get("W3300")).toBe("CGR Bordeaux - Le Français");
    expect(names.get("P0664")).toBe("CGR Villenave d'Ornon");
  });

  test("the showtimes index gives slugs", () => {
    expect(slugs).toHaveLength(73);
    expect(slugs.find((s) => s.id === "W3300")!.slug).toBe("w3300-cgr-bordeaux-le-francais");
  });

  test("theatersInCity matches the metro city in the slug, not a cinema called Le Paris", () => {
    const all = slugs.map(({ id, slug }) => ({ id: `cgr:${id}`, chain: "cgr" as const, name: names.get(id) ?? id, slug }));
    expect(theatersInCity(all, "Bordeaux").map((t) => t.id).sort()).toEqual(["cgr:P0664", "cgr:W3300"]);
    expect(theatersInCity(all, "Lyon").map((t) => t.id)).toEqual(["cgr:P0905"]);
    expect(theatersInCity(all, "Toulouse").map((t) => t.id)).toEqual(["cgr:P0692"]);
    expect(theatersInCity(all, "Paris").map((t) => t.id)).toEqual(["cgr:W7519"]);
    expect(theatersInCity(all, "Région parisienne").map((t) => t.id)).toEqual(["cgr:W7519"]);
    expect(theatersInCity(all, "Strasbourg")).toEqual([]);
  });
});

test("helpers", () => {
  expect(hm(7020)).toBe("1h57");
  expect(hm(5280)).toBe("1h28");
  expect(addSeconds("23:30", 3600)).toBe("00:30");
  expect(filmKey("DELIVRE-NOUS DU MAL")).toBe("delivre-nous-du-mal");
  expect(filmKey("Délivre-nous du mal")).toBe("delivre-nous-du-mal");
  expect(filmKey("La Pat' Patrouille : Le film mission Dino")).toBe("la-pat-patrouille-le-film-mission-dino");
  expect(filmKey("Tom & Jerry")).toBe("tom-et-jerry");
});
