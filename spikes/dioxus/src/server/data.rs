//! ~20 hard-coded sample Tests standing in for Postgres.

use crate::{SearchResults, TestRow};

const SAMPLES: &[(u16, &str, &str, &str)] = &[
    (2019, "State", "Algebra I", "Individual"),
    (2019, "State", "Geometry", "Individual"),
    (2019, "State", "Precalculus/Alpha", "Team"),
    (2019, "Regional", "Theta", "Ciphering"),
    (2020, "Invitational", "Algebra I", "Individual"),
    (2020, "Invitational", "Geometry", "Team"),
    (2020, "Mail-in", "Theta", "Logs and Exponents"),
    (2021, "State", "Algebra I", "Team"),
    (2021, "State", "Geometry", "Ciphering"),
    (2021, "Regional", "Precalculus/Alpha", "Individual"),
    (2022, "National", "Theta", "Individual"),
    (2022, "National", "Precalculus/Alpha", "Individual"),
    (2022, "Invitational", "Algebra I", "Ciphering"),
    (2023, "State", "Geometry", "Individual"),
    (2023, "State", "Theta", "Team"),
    (2023, "Regional", "Algebra I", "Individual"),
    (2024, "Invitational", "Precalculus/Alpha", "Logs and Exponents"),
    (2024, "State", "Algebra I", "Individual"),
    (2024, "State", "Geometry", "Team"),
    (2025, "Regional", "Theta", "Individual"),
    (2025, "State", "Precalculus/Alpha", "Ciphering"),
];

fn all() -> impl Iterator<Item = TestRow> {
    SAMPLES.iter().enumerate().map(|(i, (year, comp, level, ty))| TestRow {
        id: i as u32 + 1,
        year: *year,
        competition: comp.to_string(),
        level: level.to_string(),
        test_type: ty.to_string(),
    })
}

/// Empty string means "any" for a Facet.
pub fn search(year: &str, level: &str) -> SearchResults {
    let mut years: Vec<u16> = SAMPLES.iter().map(|s| s.0).collect();
    years.sort_unstable();
    years.dedup();
    let mut levels: Vec<String> = SAMPLES.iter().map(|s| s.2.to_string()).collect();
    levels.sort();
    levels.dedup();
    let tests = all()
        .filter(|t| year.is_empty() || t.year.to_string() == year)
        .filter(|t| level.is_empty() || t.level == level)
        .collect();
    SearchResults { years, levels, tests }
}
