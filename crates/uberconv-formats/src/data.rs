use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufRead, BufReader, BufWriter, Write},
};

use serde_json::{Map, Number, Value};
use uberconv_core::{Context, Converter, Cost, Edge, Error, Format, Job, Result};

/// JSON / YAML / TOML / CSV. JSON (`serde_json::Value`) is the hub, so
/// YAML -> CSV etc. fall out of the conversion graph for free.
pub struct DataConverter;

impl Converter for DataConverter {
    fn name(&self) -> &'static str {
        "data"
    }

    fn edges(&self) -> Vec<Edge> {
        use Format::*;
        vec![
            Edge::new(Json, Yaml, Cost::lossless(5)),
            Edge::new(Yaml, Json, Cost::lossless(5)),
            Edge::new(Json, Toml, Cost::lossy(5)), // no nulls, table-only root
            Edge::new(Toml, Json, Cost::lossless(5)),
            Edge::new(Json, Csv, Cost::lossy(5)), // flattens nested values to strings
            Edge::new(Csv, Json, Cost::lossless(5)),
        ]
    }

    fn convert(&self, job: &Job<'_>, ctx: &Context) -> Result<()> {
        // JSON <-> YAML never needs the whole document in memory: stream it.
        if matches!((job.from, job.to), (Format::Json, Format::Yaml) | (Format::Yaml, Format::Json)) {
            return stream_transcode(job, ctx);
        }
        let text = fs::read_to_string(job.input)
            .map_err(|e| Error::failed(format!("could not read {} as UTF-8 text: {e}", job.input.display())))?;
        ctx.report(0.3);
        let value = read_value(job.from, &text)?;
        ctx.report(0.6);
        let out = write_value(job.to, &value)?;
        fs::write(job.output, out)?;
        ctx.report(1.0);
        Ok(())
    }
}

/// JSON -> YAML and YAML -> JSON without building a DOM. JSON -> YAML is fully
/// streaming (constant memory); YAML input is still buffered by the YAML parser.
fn stream_transcode(job: &Job<'_>, ctx: &Context) -> Result<()> {
    let mut reader = BufReader::new(File::open(job.input)?);
    if reader.fill_buf()?.starts_with(&[0xEF, 0xBB, 0xBF]) {
        reader.consume(3); // UTF-8 byte order mark
    }
    let mut out = BufWriter::new(File::create(job.output)?);
    ctx.report(0.2);
    if job.from == Format::Json {
        let mut de = serde_json::Deserializer::from_reader(reader);
        let mut ser = serde_yaml::Serializer::new(&mut out);
        serde_transcode::transcode(&mut de, &mut ser).map_err(|e| Error::failed(format!("invalid JSON: {e}")))?;
        de.end().map_err(|e| Error::failed(format!("invalid JSON: {e}")))?;
    } else {
        let de = serde_yaml::Deserializer::from_reader(reader);
        let mut ser = serde_json::Serializer::with_formatter(&mut out, serde_json::ser::PrettyFormatter::new());
        serde_transcode::transcode(de, &mut ser).map_err(|e| Error::failed(format!("invalid YAML: {e}")))?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    ctx.report(1.0);
    Ok(())
}

pub fn read_value(format: Format, text: &str) -> Result<Value> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    match format {
        Format::Json => serde_json::from_str(text).map_err(|e| Error::failed(format!("invalid JSON: {e}"))),
        Format::Yaml => serde_yaml::from_str(text).map_err(|e| Error::failed(format!("invalid YAML: {e}"))),
        Format::Toml => toml::from_str(text).map_err(|e| Error::failed(format!("invalid TOML: {e}"))),
        Format::Csv => csv_to_value(text),
        other => Err(Error::UnknownFormat(other.name().into())),
    }
}

pub fn write_value(format: Format, value: &Value) -> Result<String> {
    match format {
        Format::Json => {
            let mut s = serde_json::to_string_pretty(value).map_err(|e| Error::failed(e.to_string()))?;
            s.push('\n');
            Ok(s)
        }
        Format::Yaml => serde_yaml::to_string(value).map_err(|e| Error::failed(format!("YAML: {e}"))),
        Format::Toml => {
            if !value.is_object() {
                return Err(Error::failed("TOML needs a table (object) at the top level"));
            }
            toml::to_string_pretty(value)
                .map_err(|e| Error::failed(format!("TOML can't represent this data (nulls are not allowed): {e}")))
        }
        Format::Csv => value_to_csv(value),
        other => Err(Error::UnknownFormat(other.name().into())),
    }
}

/// Best-effort typing for CSV cells. Leading zeros and `+` prefixes stay text
/// so IDs and phone numbers survive.
fn infer(cell: &str) -> Value {
    match cell {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    let keeps_text = cell.is_empty()
        || cell.starts_with('+')
        || (cell.len() > 1 && cell.starts_with('0') && !cell.starts_with("0."))
        || (cell.len() > 2 && cell.starts_with("-0") && !cell.starts_with("-0."));
    if !keeps_text {
        if let Ok(i) = cell.parse::<i64>() {
            return Value::Number(i.into());
        }
        if let Ok(f) = cell.parse::<f64>() {
            if let Some(n) = Number::from_f64(f) {
                return Value::Number(n);
            }
        }
    }
    Value::String(cell.to_string())
}

fn csv_to_value(text: &str) -> Result<Value> {
    let mut rdr = csv::ReaderBuilder::new().flexible(true).from_reader(text.as_bytes());
    let headers: Vec<String> = rdr
        .headers()
        .map_err(|e| Error::failed(format!("invalid CSV: {e}")))?
        .iter()
        .map(|h| h.to_string())
        .collect();
    let mut rows = Vec::new();
    for rec in rdr.records() {
        let rec = rec.map_err(|e| Error::failed(format!("invalid CSV: {e}")))?;
        let mut obj = Map::new();
        for (i, cell) in rec.iter().enumerate() {
            let key = headers.get(i).cloned().unwrap_or_else(|| format!("column{}", i + 1));
            obj.insert(key, infer(cell));
        }
        rows.push(Value::Object(obj));
    }
    Ok(Value::Array(rows))
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Bool(_) | Value::Number(_) => v.to_string(),
        other => other.to_string(), // nested -> compact JSON
    }
}

fn value_to_csv(value: &Value) -> Result<String> {
    let rows: Vec<&Value> = match value {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![value],
        _ => return Err(Error::failed("CSV output needs an array or object at the top level")),
    };
    let csv_err = |e: csv::Error| Error::failed(format!("CSV: {e}"));
    let mut wtr = csv::Writer::from_writer(Vec::new());

    if !rows.is_empty() && rows.iter().all(|r| r.is_object()) {
        let mut headers: Vec<String> = Vec::new();
        let mut seen: HashSet<&String> = HashSet::new();
        for r in &rows {
            for k in r.as_object().unwrap().keys() {
                if seen.insert(k) {
                    headers.push(k.clone());
                }
            }
        }
        wtr.write_record(&headers).map_err(csv_err)?;
        for r in &rows {
            let o = r.as_object().unwrap();
            let rec: Vec<String> = headers.iter().map(|h| o.get(h).map(cell).unwrap_or_default()).collect();
            wtr.write_record(&rec).map_err(csv_err)?;
        }
    } else if rows.iter().all(|r| r.is_array()) {
        for r in &rows {
            let rec: Vec<String> = r.as_array().unwrap().iter().map(cell).collect();
            wtr.write_record(&rec).map_err(csv_err)?;
        }
    } else {
        wtr.write_record(["value"]).map_err(csv_err)?;
        for r in &rows {
            wtr.write_record([cell(r)]).map_err(csv_err)?;
        }
    }
    let bytes = wtr.into_inner().map_err(|e| Error::failed(e.to_string()))?;
    String::from_utf8(bytes).map_err(|e| Error::failed(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_roundtrip_keeps_types_and_ids() {
        let v = read_value(Format::Csv, "id,name,score,zip\n1,Ann,9.5,00501\n2,Bo,10,02134\n").unwrap();
        assert_eq!(v[0]["id"], 1);
        assert_eq!(v[0]["score"], 9.5);
        assert_eq!(v[0]["zip"], "00501");
        let back = write_value(Format::Csv, &v).unwrap();
        assert!(back.starts_with("id,name,score,zip\n1,Ann,9.5,00501"));
    }

    #[test]
    fn csv_output_unions_keys_and_flattens_nested() {
        let v: Value = serde_json::from_str(r#"[{"a":1,"n":{"x":1}},{"a":2,"b":null}]"#).unwrap();
        let csv = write_value(Format::Csv, &v).unwrap();
        assert_eq!(csv.lines().next().unwrap(), "a,n,b");
        assert!(csv.contains(r#""{""x"":1}""#));
    }

    #[test]
    fn toml_rejects_non_tables() {
        assert!(write_value(Format::Toml, &Value::Array(vec![])).is_err());
    }
}
