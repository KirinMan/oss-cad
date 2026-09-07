//! Levenberg-Marquardt solving and Jacobian-rank DOF analysis (ADR-003).
//!
//! The Jacobian is numeric (central differences), not hand-derived per
//! constraint: this system is not yet driven from any hot loop (nothing in
//! the workspace calls it interactively while dragging a point — that
//! integration is future `od-app` work), so the simplicity of one generic
//! differencing routine outweighs the speed of per-constraint analytic
//! derivatives. Revisit if and when it is.

use crate::Constraint;
use crate::system::System;
use nalgebra::{DMatrix, DVector};

const MAX_ITERATIONS: usize = 100;
/// A retry budget for one Levenberg-Marquardt step: how many times `lambda`
/// is grown, at one parameter point, before giving up on that iteration.
const MAX_LAMBDA_RETRIES: usize = 30;
/// A residual vector shorter than this counts as "solved". Constraint
/// residuals mix units (mm for distances, mm² for cross products, unitless
/// ratios for angles), so this is deliberately far looser than
/// `od_geom2d::tol::POINT_EPS` — it is not a geometric tolerance, it is a
/// convergence tolerance for an iterative numerical solve.
const RESIDUAL_TOL: f64 = 1e-9;
/// A step smaller than this makes no further progress worth continuing for.
const STEP_TOL: f64 = 1e-12;
const INITIAL_LAMBDA: f64 = 1e-3;
const LAMBDA_UP: f64 = 10.0;
const LAMBDA_DOWN: f64 = 10.0;

/// What [`System::solve`] did.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveReport {
    /// Every residual is within [`RESIDUAL_TOL`] of zero.
    pub converged: bool,
    pub iterations: usize,
    pub residual_norm: f64,
}

/// What [`System::analyze`] found, from the Jacobian's numerical rank at the
/// system's current parameter values (ADR-003: "ヤコビアンの階数と残差から
/// 冗長・矛盾・過拘束を判定").
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DofReport {
    /// Total scalar parameters in the system.
    pub free_params: usize,
    /// Total residual rows every constraint contributes.
    pub constraint_rows: usize,
    /// The Jacobian's numerical rank: how many of those rows are actually
    /// independent.
    pub rank: usize,
    /// `free_params - rank`: degrees of freedom no constraint pins down.
    /// Zero means fully constrained (not necessarily satisfiable — see
    /// `conflicting`).
    pub remaining_dof: usize,
    /// `constraint_rows - rank`: rows that added no new independent
    /// information. A redundant-but-consistent constraint (the same
    /// coincidence stated twice) is harmless; this only flags that it
    /// happened, not whether it is also inconsistent.
    pub redundant: usize,
    /// The system is fully (or over-) determined (`remaining_dof == 0`) yet
    /// its residual at the current parameters is not near zero — the
    /// constraints cannot all be satisfied at once. An under-determined
    /// system that a solve failed to converge on is *not* reported here;
    /// that shows up as `SolveReport::converged == false` instead, since a
    /// nonzero `remaining_dof` alone does not mean the constraints
    /// conflict — only that more than one solution exists.
    pub conflicting: bool,
}

fn residuals_into(params: &[f64], constraints: &[Constraint], out: &mut Vec<f64>) {
    out.clear();
    for c in constraints {
        c.residuals(params, out);
    }
}

fn jacobian(params: &[f64], constraints: &[Constraint], rows: usize) -> DMatrix<f64> {
    let cols = params.len();
    let mut j = DMatrix::<f64>::zeros(rows, cols);
    let mut perturbed = params.to_vec();
    let mut r_plus = Vec::with_capacity(rows);
    let mut r_minus = Vec::with_capacity(rows);
    for col in 0..cols {
        // A step scaled to the parameter's own magnitude tracks its
        // precision at any drawing scale; the floor keeps it meaningful for
        // a parameter that is currently exactly (or near) zero.
        let h = (params[col].abs() * 1e-6).max(1e-8);
        perturbed[col] = params[col] + h;
        residuals_into(&perturbed, constraints, &mut r_plus);
        perturbed[col] = params[col] - h;
        residuals_into(&perturbed, constraints, &mut r_minus);
        perturbed[col] = params[col];
        for row in 0..rows {
            j[(row, col)] = (r_plus[row] - r_minus[row]) / (2.0 * h);
        }
    }
    j
}

pub(crate) fn solve(system: &mut System) -> SolveReport {
    let n = system.params.len();
    let m: usize = system
        .constraints
        .iter()
        .map(Constraint::residual_count)
        .sum();
    let mut buf = Vec::with_capacity(m);
    residuals_into(&system.params, &system.constraints, &mut buf);
    if m == 0 || n == 0 {
        let residual_norm = DVector::from_vec(buf).norm();
        return SolveReport {
            converged: residual_norm < RESIDUAL_TOL,
            iterations: 0,
            residual_norm,
        };
    }

    let mut lambda = INITIAL_LAMBDA;
    let mut r = DVector::from_vec(buf.clone());
    let mut norm = r.norm();

    for iter in 0..MAX_ITERATIONS {
        if norm < RESIDUAL_TOL {
            return SolveReport {
                converged: true,
                iterations: iter,
                residual_norm: norm,
            };
        }

        let j = jacobian(&system.params, &system.constraints, m);
        let jt = j.transpose();
        let jtj = &jt * &j;
        let neg_jtr = (&jt * &r).map(|v| -v);

        // Trust-region behaviour: a small lambda acts like Gauss-Newton
        // (fast near the solution), a large one like gradient descent
        // (safe far from it). Grow it until some damping level actually
        // reduces the residual, the Levenberg-Marquardt step itself.
        let mut accepted = false;
        for _ in 0..MAX_LAMBDA_RETRIES {
            let mut a = jtj.clone();
            for i in 0..n {
                a[(i, i)] += lambda;
            }
            let Some(delta) = a.lu().solve(&neg_jtr) else {
                lambda *= LAMBDA_UP;
                continue;
            };
            if delta.norm() < STEP_TOL {
                return SolveReport {
                    converged: norm < RESIDUAL_TOL,
                    iterations: iter,
                    residual_norm: norm,
                };
            }

            let mut trial = system.params.clone();
            for i in 0..n {
                trial[i] += delta[i];
            }
            residuals_into(&trial, &system.constraints, &mut buf);
            let trial_r = DVector::from_vec(buf.clone());
            let trial_norm = trial_r.norm();

            if trial_norm < norm {
                system.params = trial;
                r = trial_r;
                norm = trial_norm;
                lambda /= LAMBDA_DOWN;
                accepted = true;
                break;
            }
            lambda *= LAMBDA_UP;
        }

        if !accepted {
            // No damping level made progress: stuck at a local minimum,
            // typically a conflicting or degenerate constraint set.
            return SolveReport {
                converged: norm < RESIDUAL_TOL,
                iterations: iter,
                residual_norm: norm,
            };
        }
    }

    SolveReport {
        converged: norm < RESIDUAL_TOL,
        iterations: MAX_ITERATIONS,
        residual_norm: norm,
    }
}

pub(crate) fn analyze(system: &System) -> DofReport {
    let n = system.params.len();
    let m: usize = system
        .constraints
        .iter()
        .map(Constraint::residual_count)
        .sum();

    let mut buf = Vec::with_capacity(m);
    residuals_into(&system.params, &system.constraints, &mut buf);
    let residual_norm = DVector::from_vec(buf).norm();

    if m == 0 || n == 0 {
        return DofReport {
            free_params: n,
            constraint_rows: m,
            rank: 0,
            remaining_dof: n,
            redundant: 0,
            conflicting: false,
        };
    }

    let j = jacobian(&system.params, &system.constraints, m);
    let svd = j.svd(false, false);
    let max_sv = svd.singular_values.iter().copied().fold(0.0_f64, f64::max);
    // The standard numerical-rank convention (e.g. `numpy.linalg.matrix_rank`'s
    // own default): a singular value below this fraction of the largest one
    // reflects floating-point noise in the Jacobian, not an independent
    // constraint direction.
    let threshold = max_sv * f64::EPSILON * as_f64(n.max(m));
    let rank = svd
        .singular_values
        .iter()
        .filter(|&&s| s > threshold)
        .count();

    let remaining_dof = n.saturating_sub(rank);
    let redundant = m.saturating_sub(rank);
    let conflicting = remaining_dof == 0 && residual_norm > RESIDUAL_TOL;

    DofReport {
        free_params: n,
        constraint_rows: m,
        rank,
        remaining_dof,
        redundant,
        conflicting,
    }
}

/// `usize -> f64` without `as`'s silent precision loss (denied elsewhere in
/// the workspace for the same reason): sketches never approach `u32::MAX`
/// parameters, so the fallback never actually triggers, but going through
/// `u32` makes that a checked fact instead of an assumption.
fn as_f64(n: usize) -> f64 {
    u32::try_from(n).map_or(f64::MAX, f64::from)
}

#[cfg(test)]
mod tests {
    use crate::system::System;

    #[test]
    fn an_empty_system_is_trivially_solved() {
        let mut sys = System::new();
        let report = sys.solve();
        assert!(report.converged);
        assert_eq!(report.iterations, 0);
    }
}
