import * as hegel from "@hegeldev/hegel";
import * as gs from "@hegeldev/hegel/generators";

hegel.test((tc) => {
  const bound = gs.floats({ minValue: -1e6, maxValue: 1e6 });
  const [lo, hi] = [tc.draw(bound), tc.draw(bound)].sort((a, b) => a - b);
  const x = tc.draw(gs.floats({ minValue: lo, maxValue: hi }));
  if (x < lo || x > hi) throw new Error("point outside its interval");
}, { seed: 71, database: hegel.Database.disabled });
