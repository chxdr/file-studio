//! The list view's columns: which are shown, in what order, and how wide.
//! They live in [`crate::ViewState`] next to sort and view mode, so per-folder
//! view memory (F-012, F-021) remembers them too.

use serde::{Deserialize, Serialize};

/// A list-view column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColumnId {
    /// Icon and name. Always shown.
    Name,
    /// Logical size.
    Size,
    /// Modification date.
    Modified,
    /// Creation date.
    Created,
    /// Kind ("PNG File").
    Kind,
    /// The extension ("png").
    Extension,
}

impl ColumnId {
    /// Every column, in the default order.
    pub const ALL: [ColumnId; 6] = [
        ColumnId::Name,
        ColumnId::Size,
        ColumnId::Modified,
        ColumnId::Created,
        ColumnId::Kind,
        ColumnId::Extension,
    ];

    /// The header title.
    pub fn title(self) -> &'static str {
        match self {
            ColumnId::Name => "Name",
            ColumnId::Size => "Size",
            ColumnId::Modified => "Date Modified",
            ColumnId::Created => "Date Created",
            ColumnId::Kind => "Kind",
            ColumnId::Extension => "Extension",
        }
    }

    /// The width a new column gets, in points.
    pub fn default_width(self) -> f64 {
        match self {
            ColumnId::Name => 300.0,
            ColumnId::Size => 80.0,
            ColumnId::Modified | ColumnId::Created => 160.0,
            ColumnId::Kind => 130.0,
            ColumnId::Extension => 70.0,
        }
    }
}

/// One shown column and its width in points.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ColumnSpec {
    /// Which column.
    pub id: ColumnId,
    /// Its width in points.
    pub width: f64,
}

// Widths are always finite (see `sanitize_columns`), so equality is total.
impl Eq for ColumnSpec {}

/// The narrowest a column may be.
pub const MIN_COLUMN_WIDTH: f64 = 36.0;
/// The widest a column may be.
pub const MAX_COLUMN_WIDTH: f64 = 4000.0;

/// The default columns: Name, Size, Date Modified and Kind.
pub fn default_columns() -> Vec<ColumnSpec> {
    [
        ColumnId::Name,
        ColumnId::Size,
        ColumnId::Modified,
        ColumnId::Kind,
    ]
    .into_iter()
    .map(|id| ColumnSpec {
        id,
        width: id.default_width(),
    })
    .collect()
}

/// Makes a column list usable: no duplicates, Name always present (first
/// when it was missing), widths finite and within limits.
pub fn sanitize_columns(cols: &[ColumnSpec]) -> Vec<ColumnSpec> {
    let mut out: Vec<ColumnSpec> = Vec::with_capacity(cols.len() + 1);
    for c in cols {
        if out.iter().any(|o| o.id == c.id) {
            continue;
        }
        let width = if c.width.is_finite() {
            c.width.clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH)
        } else {
            c.id.default_width()
        };
        out.push(ColumnSpec { id: c.id, width });
    }
    if !out.iter().any(|c| c.id == ColumnId::Name) {
        out.insert(
            0,
            ColumnSpec {
                id: ColumnId::Name,
                width: ColumnId::Name.default_width(),
            },
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_keeps_order_and_fixes_bad_input() {
        let c = sanitize_columns(&[
            ColumnSpec {
                id: ColumnId::Size,
                width: f64::NAN,
            },
            ColumnSpec {
                id: ColumnId::Kind,
                width: 1.0,
            },
            ColumnSpec {
                id: ColumnId::Size,
                width: 99.0,
            },
        ]);
        let ids: Vec<ColumnId> = c.iter().map(|c| c.id).collect();
        assert_eq!(ids, [ColumnId::Name, ColumnId::Size, ColumnId::Kind]);
        assert_eq!(c[1].width, ColumnId::Size.default_width());
        assert_eq!(c[2].width, MIN_COLUMN_WIDTH);
    }

    #[test]
    fn defaults_start_with_name() {
        let d = default_columns();
        assert_eq!(d[0].id, ColumnId::Name);
        assert_eq!(sanitize_columns(&d), d);
    }
}
