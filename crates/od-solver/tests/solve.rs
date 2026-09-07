#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "an integration test asserts by panicking"
)]
//! One test per constraint kind (F-012), plus the DOF/redundancy/conflict
//! analysis ADR-003 calls for. Each constraint test pins down every degree
//! of freedom but the one or two the constraint under test is meant to
//! resolve, so a wrong residual formula shows up as a specific wrong
//! coordinate rather than "the solver didn't converge".

use od_solver::{Constraint, System};

/// Residuals mix units (mm, mm², unitless ratios), so this is a loose
/// "close enough for a converged solve" check, not a geometric tolerance.
const EPS: f64 = 1e-6;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < EPS, "{a} is not close to {b}");
}

#[test]
fn coincident_moves_the_free_point_onto_the_fixed_one() {
    let mut sys = System::new();
    let a = sys.add_point(1.0, 2.0);
    let b = sys.add_point(9.0, -3.0);
    sys.fix_point(a, 1.0, 2.0);
    sys.add_constraint(Constraint::Coincident { a, b });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    let p = sys.point(b);
    close(p.x, 1.0);
    close(p.y, 2.0);
}

#[test]
fn horizontal_zeroes_the_y_difference() {
    let mut sys = System::new();
    let a = sys.add_point(0.0, 5.0);
    let b = sys.add_point(3.0, 1.0);
    sys.fix_point(a, 0.0, 5.0);
    sys.add_constraint(Constraint::Horizontal { a, b });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    close(sys.point(b).y, 5.0);
}

#[test]
fn vertical_zeroes_the_x_difference() {
    let mut sys = System::new();
    let a = sys.add_point(4.0, 0.0);
    let b = sys.add_point(1.0, 6.0);
    sys.fix_point(a, 4.0, 0.0);
    sys.add_constraint(Constraint::Vertical { a, b });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    close(sys.point(b).x, 4.0);
}

#[test]
fn parallel_aligns_two_segment_directions() {
    let mut sys = System::new();
    let a1 = sys.add_point(0.0, 0.0);
    let a2 = sys.add_point(10.0, 0.0);
    sys.fix_point(a1, 0.0, 0.0);
    sys.fix_point(a2, 10.0, 0.0);

    let b1 = sys.add_point(0.0, 5.0);
    let b2 = sys.add_point(3.0, 8.0);
    sys.fix_point(b1, 0.0, 5.0);
    sys.add_constraint(Constraint::Parallel { a1, a2, b1, b2 });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    // a1-a2 runs along +X, so a parallel b1-b2 must end up horizontal too.
    close(sys.point(b2).y, 5.0);
}

#[test]
fn perpendicular_zeroes_the_dot_product() {
    let mut sys = System::new();
    let a1 = sys.add_point(0.0, 0.0);
    let a2 = sys.add_point(10.0, 0.0);
    sys.fix_point(a1, 0.0, 0.0);
    sys.fix_point(a2, 10.0, 0.0);

    let b1 = sys.add_point(2.0, 2.0);
    let b2 = sys.add_point(6.0, 5.0);
    sys.fix_point(b1, 2.0, 2.0);
    sys.add_constraint(Constraint::Perpendicular { a1, a2, b1, b2 });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    // a1-a2 runs along +X, so a perpendicular b1-b2 must end up vertical.
    close(sys.point(b2).x, 2.0);
}

#[test]
fn equal_length_matches_two_segment_lengths() {
    let mut sys = System::new();
    let a1 = sys.add_point(0.0, 0.0);
    let a2 = sys.add_point(6.0, 0.0);
    sys.fix_point(a1, 0.0, 0.0);
    sys.fix_point(a2, 6.0, 0.0);

    let b1 = sys.add_point(0.0, 0.0);
    let b2 = sys.add_point(1.0, 1.0);
    sys.fix_point(b1, 0.0, 0.0);
    sys.add_constraint(Constraint::EqualLength { a1, a2, b1, b2 });
    // Pin the direction too, so the length constraint has one clean answer.
    sys.add_constraint(Constraint::Horizontal { a: b1, b: b2 });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    let p = sys.point(b2);
    close(p.x, 6.0);
    close(p.y, 0.0);
}

#[test]
fn concentric_matches_two_circle_centers() {
    let mut sys = System::new();
    let a_center = sys.add_point(5.0, 5.0);
    sys.fix_point(a_center, 5.0, 5.0);
    let a = sys.add_circle(a_center, 3.0);

    let b_center = sys.add_point(0.0, 0.0);
    let b = sys.add_circle(b_center, 1.0);
    sys.add_constraint(Constraint::Concentric { a, b });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    let p = sys.point(b.center);
    close(p.x, 5.0);
    close(p.y, 5.0);
}

#[test]
fn symmetric_mirrors_a_point_across_an_axis() {
    let mut sys = System::new();
    let axis1 = sys.add_point(0.0, 0.0);
    let axis2 = sys.add_point(10.0, 0.0);
    sys.fix_point(axis1, 0.0, 0.0);
    sys.fix_point(axis2, 10.0, 0.0);

    let a = sys.add_point(2.0, 3.0);
    sys.fix_point(a, 2.0, 3.0);
    let b = sys.add_point(1.0, 1.0);
    sys.add_constraint(Constraint::Symmetric { a, b, axis1, axis2 });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    let p = sys.point(b);
    close(p.x, 2.0);
    close(p.y, -3.0);
}

#[test]
fn fixed_pins_a_single_parameter() {
    let mut sys = System::new();
    let x = sys.add_param(0.0);
    sys.add_constraint(Constraint::Fixed {
        param: x,
        target: 42.0,
    });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    close(sys.value(x), 42.0);
}

#[test]
fn tangent_circles_touch_externally() {
    let mut sys = System::new();
    let a_center = sys.add_point(0.0, 0.0);
    sys.fix_point(a_center, 0.0, 0.0);
    let a = sys.add_circle(a_center, 3.0);
    sys.add_constraint(Constraint::Fixed {
        param: a.radius,
        target: 3.0,
    });

    let b_center = sys.add_point(6.0, 1.0);
    let b = sys.add_circle(b_center, 2.0);
    sys.add_constraint(Constraint::Fixed {
        param: b.radius,
        target: 2.0,
    });
    // Keep b's centre on the same horizontal as a's, so tangency alone
    // pins down the remaining free coordinate.
    sys.add_constraint(Constraint::Horizontal {
        a: a_center,
        b: b_center,
    });
    sys.add_constraint(Constraint::TangentCircles { a, b });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    let p = sys.point(b.center);
    close(p.x, 5.0);
    close(p.y, 0.0);
}

#[test]
fn tangent_line_circle_touches_at_the_radius_distance() {
    let mut sys = System::new();
    let p1 = sys.add_point(0.0, 0.0);
    let p2 = sys.add_point(10.0, 0.0);
    sys.fix_point(p1, 0.0, 0.0);
    sys.fix_point(p2, 10.0, 0.0);

    let center = sys.add_point(5.0, 3.0);
    sys.add_constraint(Constraint::Fixed {
        param: center.x,
        target: 5.0,
    });
    let circle = sys.add_circle(center, 2.0);
    sys.add_constraint(Constraint::Fixed {
        param: circle.radius,
        target: 2.0,
    });
    sys.add_constraint(Constraint::TangentLineCircle { p1, p2, circle });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    // p1-p2 runs along the X axis, so tangency puts the centre exactly one
    // radius above it (the initial guess's own side).
    close(sys.point(center).y, 2.0);
}

#[test]
fn distance_sets_the_separation_between_two_points() {
    let mut sys = System::new();
    let a = sys.add_point(0.0, 0.0);
    sys.fix_point(a, 0.0, 0.0);
    let b = sys.add_point(3.0, 1.0);
    sys.add_constraint(Constraint::Horizontal { a, b });
    sys.add_constraint(Constraint::Distance { a, b, target: 7.0 });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    let p = sys.point(b);
    close(p.x, 7.0);
    close(p.y, 0.0);
}

#[test]
fn angle_sets_the_sweep_between_two_segments() {
    let mut sys = System::new();
    let a1 = sys.add_point(0.0, 0.0);
    let a2 = sys.add_point(10.0, 0.0);
    sys.fix_point(a1, 0.0, 0.0);
    sys.fix_point(a2, 10.0, 0.0);

    let b1 = sys.add_point(0.0, 0.0);
    sys.fix_point(b1, 0.0, 0.0);
    let b2 = sys.add_point(3.0, 4.0);
    sys.add_constraint(Constraint::Angle {
        a1,
        a2,
        b1,
        b2,
        target: std::f64::consts::FRAC_PI_2,
    });
    sys.add_constraint(Constraint::Distance {
        a: b1,
        b: b2,
        target: 5.0,
    });

    let report = sys.solve();
    assert!(report.converged, "{report:?}");
    // a1-a2 runs along +X (angle 0); +90° from that is straight up.
    let p = sys.point(b2);
    close(p.x, 0.0);
    close(p.y, 5.0);
}

#[test]
fn analyze_reports_remaining_dof_for_an_under_constrained_system() {
    let mut sys = System::new();
    let a = sys.add_point(0.0, 0.0);
    let b = sys.add_point(5.0, 5.0);
    sys.add_constraint(Constraint::Coincident { a, b });

    let report = sys.analyze();
    assert_eq!(report.free_params, 4);
    assert_eq!(report.constraint_rows, 2);
    assert_eq!(report.rank, 2);
    assert_eq!(report.remaining_dof, 2);
    assert_eq!(report.redundant, 0);
    assert!(!report.conflicting);
}

#[test]
fn analyze_flags_redundant_but_not_conflicting_when_the_duplicate_agrees() {
    let mut sys = System::new();
    let x = sys.add_param(0.0);
    let y = sys.add_param(0.0);
    sys.add_constraint(Constraint::Fixed {
        param: x,
        target: 5.0,
    });
    sys.add_constraint(Constraint::Fixed {
        param: x,
        target: 5.0,
    });
    sys.add_constraint(Constraint::Fixed {
        param: y,
        target: 1.0,
    });

    let solved = sys.solve();
    assert!(solved.converged, "{solved:?}");

    let report = sys.analyze();
    assert_eq!(report.free_params, 2);
    assert_eq!(report.constraint_rows, 3);
    assert_eq!(report.remaining_dof, 0);
    assert_eq!(report.redundant, 1, "the duplicate Fixed adds no new rank");
    assert!(!report.conflicting, "both Fixed rows agree");
}

#[test]
fn analyze_flags_conflicting_when_the_duplicate_disagrees() {
    let mut sys = System::new();
    let x = sys.add_param(0.0);
    let y = sys.add_param(0.0);
    sys.add_constraint(Constraint::Fixed {
        param: x,
        target: 5.0,
    });
    sys.add_constraint(Constraint::Fixed {
        param: x,
        target: 10.0,
    });
    sys.add_constraint(Constraint::Fixed {
        param: y,
        target: 1.0,
    });

    // Not expected to converge: the two Fixed rows on `x` cannot both be
    // zero at once. The solver still settles (least-squares) at the
    // midpoint, which is exactly the "conflicting" case analyze() reports.
    sys.solve();

    let report = sys.analyze();
    assert_eq!(report.remaining_dof, 0);
    assert_eq!(report.redundant, 1);
    assert!(report.conflicting);
}
