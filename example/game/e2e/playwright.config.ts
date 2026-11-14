/**
 * Browser tests of the example game. The config starts its own game server
 * (`cargo run` in ../server) on a spare port with a fresh PostgreSQL
 * database, so a run never touches the development data. It needs the dev
 * database server (`mise run game:db:up`, or `E2E_DATABASE_URL` pointing at
 * another server whose role may create databases) and the built web client
 * (`mise run game:e2e` does all three).
 */
import { defineConfig, devices } from '@playwright/test';

const port = Number(process.env.E2E_PORT ?? 18091);
// Workers re-evaluate this file: the first evaluation picks the database
// and the environment carries it to the rest. The server creates it
// (`DATABASE_CREATE=1`). The `gs_e2e_<unix secs>` name lets the server's
// test runs (`cargo test`, `db::testing`) drop it once it is six hours old.
const server = process.env.E2E_DATABASE_URL ?? 'postgres://mobbed:mobbed@127.0.0.1:55432/mobbed';
process.env.E2E_DB ??= `gs_e2e_${Math.floor(Date.now() / 1000)}`;
const databaseUrl = server.replace(/\/[^/?]*(\?|$)/, `/${process.env.E2E_DB}$1`);

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
      DATABASE_URL: databaseUrl,
      DATABASE_CREATE: '1',
      RUST_LOG: 'warn',
      // Test hooks: new characters start at level 30 (tier-2 skills), and
      // `/?mission=<id>` picks a mission without walking to the board.
      GAME_DEV: '1',
      GAME_DEV_LEVEL: '30',
    },
  },
});
