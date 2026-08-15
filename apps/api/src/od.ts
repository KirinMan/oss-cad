import { type z } from 'zod';

/**
 * Runs the `od` binary and validates what comes back.
 *
 * The CLI is the API's worker rather than a reimplementation of it (ADR-006,
 * `docs/02-architecture.md`): drawing logic lives in one place, in Rust, and
 * this layer is transport. That also means every code path the API exposes is
 * one a user can run themselves, which makes bug reports reproducible.
 */

/** Where to find the binary. Overridable so CI can point at a release build. */
const OD_BIN = process.env.OD_BIN ?? 'od';

/** Anything slower than this is a runaway, not a big drawing. */
const TIMEOUT_MS = Number(process.env.OD_TIMEOUT_MS ?? 60_000);

export class OdError extends Error {
  constructor(
    message: string,
    readonly code: number,
    readonly stderr: string,
  ) {
    super(message);
    this.name = 'OdError';
  }
}

export class OdMissingError extends Error {
  constructor(readonly binary: string) {
    super(
      `the \`${binary}\` binary was not found. Build it with \`cargo build --release -p od-cli\` ` +
        `and put it on PATH, or set OD_BIN to its location.`,
    );
    this.name = 'OdMissingError';
  }
}

/** Runs `od --json <args>` and parses stdout against `schema`. */
export async function od<T>(schema: z.ZodType<T>, args: string[]): Promise<T> {
  const proc = Bun.spawn([OD_BIN, '--json', ...args], {
    stdout: 'pipe',
    stderr: 'pipe',
  });

  const timeout = setTimeout(() => proc.kill(), TIMEOUT_MS);
  let stdout: string;
  let stderr: string;
  let code: number;
  try {
    [stdout, stderr, code] = await Promise.all([
      new Response(proc.stdout).text(),
      new Response(proc.stderr).text(),
      proc.exited,
    ]);
  } catch (cause) {
    // Bun reports a missing executable when the process is awaited, not when
    // it is spawned, so the friendly message belongs here.
    if (cause instanceof Error && /ENOENT|not found/i.test(cause.message)) {
      throw new OdMissingError(OD_BIN);
    }
    throw cause;
  } finally {
    clearTimeout(timeout);
  }

  // `check` and `roundtrip` exit non-zero to signal findings, and their output
  // is still valid — the report is the point. A parse failure below tells the
  // two cases apart.
  const trimmed = stdout.trim();
  if (trimmed === '') {
    if (/no such file|not found/i.test(stderr)) {
      throw new OdMissingError(OD_BIN);
    }
    throw new OdError(stderr.trim() || `od exited with code ${code}`, code, stderr);
  }

  let parsed: unknown;
  try {
    parsed = JSON.parse(trimmed);
  } catch {
    throw new OdError(
      `od produced output that is not JSON: ${trimmed.slice(0, 200)}`,
      code,
      stderr,
    );
  }

  const result = schema.safeParse(parsed);
  if (!result.success) {
    // A schema mismatch means the CLI and this service have drifted. Say so
    // plainly rather than passing half-understood data to the client.
    throw new OdError(
      `od output did not match the expected shape: ${result.error.issues
        .map((i) => `${i.path.join('.')}: ${i.message}`)
        .join('; ')}`,
      code,
      stderr,
    );
  }
  return result.data;
}

/** True when the binary is present and runnable. */
export async function odAvailable(): Promise<boolean> {
  try {
    const proc = Bun.spawn([OD_BIN, '--version'], { stdout: 'pipe', stderr: 'pipe' });
    const code = await proc.exited;
    return code === 0;
  } catch {
    return false;
  }
}

export const odBinary = OD_BIN;
