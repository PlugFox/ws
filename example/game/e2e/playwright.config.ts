/**
 * Browser tests of the example game. The config starts its own game server
 * (`cargo run` in ../server) on a spare port with a fresh SQLite file, so a
 * run never touches the development database. The web client must be built
 * first (`mise run game:e2e` does both).
 */
import { defineConfig, devices } from '@playwright/test';
import os from 'node:os';
import path from 'node:path';

const port = Number(process.env.E2E_PORT ?? 18091);
// Workers re-evaluate this file: the first evaluation picks the database
// and the environment carries it to the rest.
process.env.E2E_DB ??= path.join(os.tmpdir(), `ws-game-e2e-${Date.now()}.db`);

export default defineConfig({
  testDir: './tests',
  timeout: 90_000,
  expect: { timeout: 20_000 },
  // One server, one world: tests share it, so they run one at a time.
  workers: 1,
  fullyParallel: false,
  forbidOnly: !!process.env.CI,
  // The HTML report (`mise run game:e2e:report`) keeps traces of failures.
  reporter: [[process.env.CI ? 'github' : 'list'], ['html', { open: 'never' }]],
  use: {
    baseURL: `http://127.0.0.1:${port}`,
    viewport: { width: 1280, height: 800 },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
    video: 'retain-on-failure',
  },
  projects: [
    {
      name: 'chromium',
      use: {
        ...devices['Desktop Chrome'],
        viewport: { width: 1280, height: 800 },
        // WebGL2 without a GPU (CI): SwiftShader through ANGLE.
        launchOptions: { args: ['--use-angle=swiftshader', '--enable-unsafe-swiftshader', '--mute-audio'] },
      },
    },
  ],
  webServer: {
    command: 'cargo run --quiet',
    cwd: '../server',
    url: `http://127.0.0.1:${port}/`,
    // The first run compiles the server.
    timeout: 600_000,
    reuseExistingServer: false,
    stdout: 'ignore',
    stderr: 'pipe',
    env: {
      SERVER_PORT: String(port),
      DATABASE_URL: `sqlite://${process.env.E2E_DB}?mode=rwc`,
      RUST_LOG: 'warn',
    },
  },
});
