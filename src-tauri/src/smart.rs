//! Smart (rule-based) playlist evaluation. Rules are compiled to a
//! parameterised SQL query; field names are whitelisted.

use anyhow::{Result, bail};
use rusqlite::{Connection, types::Value};

use crate::models::{SmartRule, SmartRules, now_secs};

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Num,
    Date,
}

fn column(field: &str) -> Option<(&'static str, Kind)> {
    Some(match field {
        "title" => ("title", Kind::Text),
        "artist" => ("artist", Kind::Text),
        "albumArtist" => ("album_artist", Kind::Text),
        "album" => ("album", Kind::Text),
        "genre" => ("genre", Kind::Text),
        "composer" => ("composer", Kind::Text),
        "comment" => ("comment", Kind::Text),
        "format" => ("format", Kind::Text),
        "path" => ("path", Kind::Text),
        "year" => ("year", Kind::Num),
        "trackNo" => ("track_no", Kind::Num),
        "discNo" => ("disc_no", Kind::Num),
        "duration" => ("duration", Kind::Num),
        "rating" => ("rating", Kind::Num),
        "loved" => ("loved", Kind::Num),
        "playCount" => ("play_count", Kind::Num),
        "bitrate" => ("bitrate", Kind::Num),
        "bitDepth" => ("bit_depth", Kind::Num),
        "dateAdded" => ("date_added", Kind::Date),
        "lastPlayed" => ("last_played", Kind::Date),
        _ => return None,
    })
}

fn compile_rule(rule: &SmartRule, params: &mut Vec<Value>) -> Result<String> {
    let Some((col, kind)) = column(&rule.field) else {
        bail!("unknown field {}", rule.field);
    };
    let v = rule.value.trim();
    let sql = match (kind, rule.op.as_str()) {
        (Kind::Text, "contains") => {
            params.push(Value::Text(v.to_lowercase()));
            format!("instr(lower({col}), ?) > 0")
        }
        (Kind::Text, "notContains") => {
            params.push(Value::Text(v.to_lowercase()));
            format!("instr(lower({col}), ?) = 0")
        }
        (Kind::Text, "is") => {
            params.push(Value::Text(v.into()));
            format!("{col} = ? COLLATE NOCASE")
        }
        (Kind::Text, "isNot") => {
            params.push(Value::Text(v.into()));
            format!("{col} <> ? COLLATE NOCASE")
        }
        (Kind::Text, "startsWith") => {
            params.push(Value::Text(v.to_lowercase()));
            format!("instr(lower({col}), ?) = 1")
        }
        (Kind::Num, op) => {
            let mut n: f64 = v.parse().unwrap_or(0.0);
            // Rules are written in stars (3.5); the column holds half-stars (7).
            if col == "rating" {
                n *= 2.0;
            }
            params.push(Value::Real(n));
            let cmp = match op {
                "is" => "=",
                "isNot" => "<>",
                "gt" => ">",
                "lt" => "<",
                _ => bail!("bad operator {op} for {}", rule.field),
            };
            format!("IFNULL({col}, 0) {cmp} ?")
        }
        (Kind::Date, op) => {
            let days: f64 = v.parse().unwrap_or(0.0);
            let since = now_secs() - (days * 86400.0) as i64;
            params.push(Value::Integer(since));
            match op {
                "inLast" => format!("IFNULL({col}, 0) >= ?"),
                "notInLast" => format!("IFNULL({col}, 0) < ?"),
                _ => bail!("bad operator {op} for {}", rule.field),
            }
        }
        (_, op) => bail!("bad operator {op} for {}", rule.field),
    };
    Ok(sql)
}

pub fn evaluate(conn: &Connection, rules: &SmartRules) -> Result<Vec<i64>> {
    let mut params = Vec::new();
    let mut clauses = Vec::new();
    for r in &rules.rules {
        clauses.push(compile_rule(r, &mut params)?);
    }
    let joiner = if rules.r#match == "any" { " OR " } else { " AND " };
    let where_sql = if clauses.is_empty() { "1".to_string() } else { clauses.join(joiner) };

    let order = match rules.sort_by.as_deref() {
        Some("random") => "RANDOM()".to_string(),
        Some(f) => match column(f) {
            Some((col, _)) => format!("{col} {}", if rules.sort_desc { "DESC" } else { "ASC" }),
            None => "artist, album, disc_no, track_no".into(),
        },
        None => "artist, album, disc_no, track_no".into(),
    };
    let limit = rules.limit.map(|l| format!(" LIMIT {l}")).unwrap_or_default();

    // Inbox tracks haven't been added to the library yet.
    let sql = format!("SELECT id FROM tracks WHERE inbox = 0 AND ({where_sql}) ORDER BY {order}{limit}");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| r.get(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::models::Track;

    fn lib() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        db::migrate(&conn).unwrap();
        let add = |path: &str, artist: &str, genre: &str, year: u32, rating: u8| {
            let t = Track {
                path: path.into(),
                title: path.into(),
                artist: artist.into(),
                genre: genre.into(),
                year: Some(year),
                date_added: now_secs(),
                ..Default::default()
            };
            let id = db::upsert_track(&conn, &t).unwrap();
            conn.execute("UPDATE tracks SET rating = ?2 WHERE id = ?1", rusqlite::params![id, rating * 2])
                .unwrap();
        };
        add("a", "Alpha Bees", "Electronic", 2021, 5);
        add("b", "Alpha Bees", "Electronic", 2015, 2);
        add("c", "Comb Theory", "Jazz", 2019, 4);
        add("d", "Drone Unit", "Rock", 2023, 0);
        conn
    }

    fn rules(m: &str, rules: &[(&str, &str, &str)]) -> SmartRules {
        SmartRules {
            r#match: m.into(),
            rules: rules
                .iter()
                .map(|(f, o, v)| SmartRule { field: (*f).into(), op: (*o).into(), value: (*v).into() })
                .collect(),
            limit: None,
            sort_by: Some("title".into()),
            sort_desc: false,
        }
    }

    fn titles(conn: &Connection, r: &SmartRules) -> Vec<String> {
        evaluate(conn, r)
            .unwrap()
            .into_iter()
            .map(|id| db::track_by_id(conn, id).unwrap().unwrap().title)
            .collect()
    }

    #[test]
    fn match_all_and_any() {
        let conn = lib();
        let all = rules("all", &[("artist", "contains", "alpha"), ("rating", "gt", "3")]);
        assert_eq!(titles(&conn, &all), ["a"]);
        let any = rules("any", &[("genre", "is", "jazz"), ("year", "gt", "2022")]);
        assert_eq!(titles(&conn, &any), ["c", "d"]);
    }

    #[test]
    fn inbox_tracks_are_left_out() {
        let conn = lib();
        conn.execute("UPDATE tracks SET inbox = 1 WHERE path = 'c'", []).unwrap();
        assert_eq!(titles(&conn, &rules("all", &[])), ["a", "b", "d"]);
    }

    #[test]
    fn negation_limit_and_sort() {
        let conn = lib();
        let mut r = rules("all", &[("genre", "notContains", "rock")]);
        r.sort_by = Some("year".into());
        r.sort_desc = true;
        r.limit = Some(2);
        assert_eq!(titles(&conn, &r), ["a", "c"]);
    }

    #[test]
    fn date_rules_and_bad_input() {
        let conn = lib();
        assert_eq!(titles(&conn, &rules("all", &[("dateAdded", "inLast", "7")])).len(), 4);
        assert!(titles(&conn, &rules("all", &[("lastPlayed", "inLast", "7")])).is_empty());
        // Unknown fields are rejected rather than interpolated into SQL.
        assert!(evaluate(&conn, &rules("all", &[("title; DROP TABLE tracks", "is", "x")])).is_err());
        assert!(evaluate(&conn, &rules("all", &[("year", "contains", "1")])).is_err());
    }
}
