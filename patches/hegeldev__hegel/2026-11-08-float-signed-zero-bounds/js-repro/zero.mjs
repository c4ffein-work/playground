import * as hegel from "@hegeldev/hegel";
import * as gs from "@hegeldev/hegel/generators";

hegel.test((tc) => {
  tc.draw(gs.floats({ minValue: 0, maxValue: -0 }));
});
