import { defineConfig } from 'vitest/config';

// Contract tests: the real ApiClient against a real running Django backend.
// Deliberately a separate config so `npm run test:unit` (vitest.config.js,
// which includes only tests/unit/**) never picks these up.
// Run via `npm run test:contract` or `bash scripts/test-contract.sh`.
export default defineConfig({
  test: {
    environment: 'node',
    include: ['tests/contract/**/*.{test,spec}.js'],
    // Tests within the suite share backend state and must run in order,
    // against a single backend instance.
    fileParallelism: false,
    testTimeout: 15000,
    hookTimeout: 15000,
  },
});
