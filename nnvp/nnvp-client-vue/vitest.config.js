import { defineConfig } from 'vitest/config';

// Vitest is only used for the pure-logic unit tests under tests/unit.
// The rest of tests/ holds Playwright e2e specs, which must NOT be picked up here.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/unit/**/*.{test,spec}.js'],
  },
});
