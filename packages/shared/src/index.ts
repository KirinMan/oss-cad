/**
 * Types and runtime schemas shared by the API and the front end.
 *
 * Everything here mirrors a contract the Rust side produces. Keeping the
 * schemas — not just the types — on this side means a change in the CLI's
 * output is caught at the boundary with a useful message, instead of surfacing
 * three layers later as `undefined is not an object`.
 */

export * from './drawing.ts';
export * from './parts.ts';

/** The API's error shape. One field, so clients have nothing to guess at. */
export interface ApiError {
  error: string;
  detail?: string;
}
