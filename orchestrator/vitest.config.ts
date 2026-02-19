import { defineConfig } from 'vitest/config';

export default defineConfig({
  test: {
    clearMocks: true,
    unstubEnvs: true,
    restoreMocks: true,
    testTimeout: 30_000,
    include: ['tests/**/*.test.ts'],
  },
});
