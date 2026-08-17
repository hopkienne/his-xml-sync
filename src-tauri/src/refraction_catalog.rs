use serde::Deserialize;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
struct CatalogEntry {
    id: i64,
    name: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct AddEntry {
    id: i64,
    name: String,
}

#[derive(Debug, Clone)]
enum Lookup {
    Id(i64),
    Ambiguous(Vec<i64>),
}

type LookupTable = HashMap<String, Lookup>;

pub struct Catalog {
    sph: LookupTable,
    cyl: LookupTable,
    axis: LookupTable,
    visual_acuity: LookupTable,
    add: LookupTable,
}

pub fn catalog() -> Result<&'static Catalog, String> {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    if let Some(catalog) = CATALOG.get() {
        return Ok(catalog);
    }

    let entries: Vec<CatalogEntry> =
        serde_json::from_str(include_str!("../resources/dm_khuc_xa_hcm.json"))
            .map_err(|error| format!("Danh mục khúc xạ HCM không hợp lệ: {error}"))?;
    let add_entries: Vec<AddEntry> =
        serde_json::from_str(include_str!("../resources/dm_don_vi_add_hcm.json"))
            .map_err(|error| format!("Danh mục đơn vị ADD HCM không hợp lệ: {error}"))?;

    let mut parsed = Catalog {
        sph: LookupTable::new(),
        cyl: LookupTable::new(),
        axis: LookupTable::new(),
        visual_acuity: LookupTable::new(),
        add: LookupTable::new(),
    };

    for entry in entries {
        match entry.kind.as_str() {
            "SPH" => insert_numeric(&mut parsed.sph, &entry.name, entry.id, "SPH")?,
            "CYL" => insert_numeric(&mut parsed.cyl, &entry.name, entry.id, "CYL")?,
            "Axis" => insert_numeric(&mut parsed.axis, &entry.name, entry.id, "Axis")?,
            // Thị lực giữ nguyên chuỗi trong cột `ten`: không quy đổi 20/200 thành 0.1.
            "Thị lực" => insert(&mut parsed.visual_acuity, text_key(&entry.name), entry.id),
            _ => {}
        }
    }
    for entry in add_entries {
        insert_numeric(&mut parsed.add, &entry.name, entry.id, "ADD")?;
    }

    let _ = CATALOG.set(parsed);
    CATALOG
        .get()
        .ok_or_else(|| "Không khởi tạo được danh mục khúc xạ HCM.".into())
}

pub fn sph_id(catalog: &Catalog, value: f64) -> Result<i64, String> {
    lookup_numeric(&catalog.sph, value, "SPH")
}

pub fn cyl_id(catalog: &Catalog, value: f64) -> Result<i64, String> {
    lookup_numeric(&catalog.cyl, value, "CYL")
}

pub fn axis_id(catalog: &Catalog, value: f64) -> Result<i64, String> {
    lookup_numeric(&catalog.axis, value, "Axis")
}

pub fn add_id(catalog: &Catalog, value: &str) -> Result<i64, String> {
    lookup_numeric_text(&catalog.add, value, "ADD")
}

pub fn sph_id_from_text(catalog: &Catalog, value: &str) -> Result<i64, String> {
    lookup_numeric_text(&catalog.sph, value, "SPH")
}

pub fn cyl_id_from_text(catalog: &Catalog, value: &str) -> Result<i64, String> {
    lookup_numeric_text(&catalog.cyl, value, "CYL")
}

pub fn axis_id_from_text(catalog: &Catalog, value: &str) -> Result<i64, String> {
    lookup_numeric_text(&catalog.axis, value, "Axis")
}

pub fn visual_acuity_id(catalog: &Catalog, value: &str) -> Result<i64, String> {
    lookup(&catalog.visual_acuity, &text_key(value), "Thị lực", value)
}

fn insert_numeric(table: &mut LookupTable, value: &str, id: i64, kind: &str) -> Result<(), String> {
    let key = numeric_key(value)
        .map_err(|_| format!("Danh mục {kind} có giá trị không phải số: {value}"))?;
    insert(table, key, id);
    Ok(())
}

fn insert(table: &mut LookupTable, key: String, id: i64) {
    if let Some(existing) = table.get_mut(&key) {
        match existing {
            Lookup::Id(previous) if *previous != id => {
                let prior_id = *previous;
                *existing = Lookup::Ambiguous(vec![prior_id, id]);
            }
            Lookup::Ambiguous(ids) if !ids.contains(&id) => ids.push(id),
            _ => {}
        }
    } else {
        table.insert(key, Lookup::Id(id));
    }
}

fn lookup_numeric(table: &LookupTable, value: f64, kind: &str) -> Result<i64, String> {
    if !value.is_finite() {
        return Err(format!("Giá trị {kind} không hợp lệ: {value}"));
    }
    lookup(
        table,
        &numeric_key_from_f64(value),
        kind,
        &value.to_string(),
    )
}

fn lookup_numeric_text(table: &LookupTable, value: &str, kind: &str) -> Result<i64, String> {
    let key = numeric_key(value).map_err(|_| format!("Giá trị {kind} không phải số: {value}"))?;
    lookup(table, &key, kind, value)
}

fn lookup(table: &LookupTable, key: &str, kind: &str, raw: &str) -> Result<i64, String> {
    match table.get(key) {
        Some(Lookup::Id(id)) => Ok(*id),
        Some(Lookup::Ambiguous(ids)) => Err(format!(
            "Danh mục {kind} trùng giá trị {raw}: các ID {}",
            ids.iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )),
        None => Err(format!("Không tìm thấy danh mục {kind} cho giá trị {raw}")),
    }
}

fn numeric_key(value: &str) -> Result<String, ()> {
    let parsed = value.trim().parse::<f64>().map_err(|_| ())?;
    if !parsed.is_finite() {
        return Err(());
    }
    Ok(numeric_key_from_f64(parsed))
}

fn numeric_key_from_f64(value: f64) -> String {
    // 6 chữ số thập phân đủ cho các danh mục hiện có và vẫn giữ dấu âm.
    format!("{:.6}", if value == -0.0 { 0.0 } else { value })
}

fn text_key(value: &str) -> String {
    value.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_sign_for_sphere_and_cylinder() {
        let catalog = catalog().unwrap();
        assert_eq!(sph_id_from_text(catalog, "-2.50").unwrap(), 1100);
        assert_eq!(sph_id_from_text(catalog, "+2.50").unwrap(), 1076);
        assert_eq!(cyl_id_from_text(catalog, "-0.75").unwrap(), 1335);
        assert_eq!(cyl_id_from_text(catalog, "+0.75").unwrap(), 1295);
    }

    #[test]
    fn visual_acuity_uses_the_exact_catalogue_name() {
        let catalog = catalog().unwrap();
        assert_eq!(visual_acuity_id(catalog, "20/200").unwrap(), 852);
        assert_eq!(visual_acuity_id(catalog, "1/10").unwrap(), 1902);
        assert_eq!(visual_acuity_id(catalog, "ĐNT 0.1m").unwrap(), 887);
        assert_eq!(visual_acuity_id(catalog, "ST(+)").unwrap(), 893);
    }

    #[test]
    fn duplicate_catalogue_values_are_rejected_on_lookup() {
        let catalog = catalog().unwrap();
        let error = visual_acuity_id(catalog, "1.0").unwrap_err();
        assert!(error.contains("861") && error.contains("1752"));
        let error = sph_id_from_text(catalog, "+6.75").unwrap_err();
        assert!(error.contains("1853"));
    }
}
