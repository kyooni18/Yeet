use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::Read,
    path::Path,
};

use anyhow::{Context, Result, anyhow, bail};
use calamine::{Reader as CalamineReader, open_workbook_auto};
use quick_xml::{Reader as XmlReader, escape::unescape, events::Event};
use serde_json::{Map, Value, json};
use zip::ZipArchive;

const MAX_DATA_ROWS: usize = 250_000;
const MAX_DATA_COLUMNS: usize = 512;
const MAX_JSON_BYTES: u64 = 128 * 1024 * 1024;
const SPREADSHEET_DOCUMENT_ROWS: usize = 250;
const SPREADSHEET_DOCUMENT_COLUMNS: usize = 64;

#[derive(Debug, Clone)]
pub struct DocumentContent {
    pub kind: String,
    pub media_type: String,
    pub text: String,
    pub metadata: Value,
}

#[derive(Debug, Clone)]
struct DataSet {
    source_kind: String,
    sheet: Option<String>,
    columns: Vec<String>,
    rows: Vec<Vec<Option<String>>>,
}

pub fn read_document(path: &Path) -> Result<DocumentContent> {
    if !path.is_file() {
        bail!("document path is not a file: {}", path.display());
    }
    let extension = extension(path);
    match extension.as_str() {
        "pdf" => {
            let text = pdf_extract::extract_text(path)
                .with_context(|| format!("failed to extract PDF text from {}", path.display()))?;
            Ok(DocumentContent {
                kind: "pdf".into(),
                media_type: "application/pdf".into(),
                metadata: json!({"characters": text.chars().count(), "lines": text.lines().count()}),
                text,
            })
        }
        "docx" => read_docx(path),
        "xlsx" | "xls" | "xlsb" | "ods" => read_spreadsheet_document(path),
        "csv" | "tsv" => read_delimited_document(path, extension == "tsv"),
        "json" | "jsonl" | "ndjson" => {
            let text = fs::read_to_string(path)
                .with_context(|| format!("failed to read {} as UTF-8", path.display()))?;
            Ok(DocumentContent {
                kind: "json".into(),
                media_type: "application/json".into(),
                metadata: json!({"characters": text.chars().count(), "lines": text.lines().count()}),
                text,
            })
        }
        "md" | "markdown" | "txt" | "text" | "log" | "rtf" | "html" | "htm" | "xml"
        | "yaml" | "yml" | "toml" => read_plain_document(path, &extension),
        _ => read_plain_document(path, &extension).map_err(|_| {
            anyhow!(
                "unsupported document format .{} for {}; supported formats include PDF, DOCX, XLS/XLSX/ODS, CSV/TSV, JSON, Markdown, and UTF-8 text",
                if extension.is_empty() { "?" } else { &extension },
                path.display()
            )
        }),
    }
}

pub fn analyze_data(path: &Path, options: &Map<String, Value>) -> Result<Value> {
    let sheet = options.get("sheet").and_then(Value::as_str);
    let dataset = load_dataset(path, sheet)?;
    let operation = options
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("describe");
    let mut result = match operation {
        "describe" => describe_dataset(&dataset),
        "value_counts" => value_counts(
            &dataset,
            required_option(options, "column", "value_counts")?,
            usize_option(options, "maxGroups")
                .unwrap_or(20)
                .clamp(1, 100),
        )?,
        "group_by" => group_by(
            &dataset,
            required_option(options, "groupBy", "group_by")?,
            options.get("valueColumn").and_then(Value::as_str),
            options
                .get("aggregation")
                .and_then(Value::as_str)
                .unwrap_or("count"),
            usize_option(options, "maxGroups")
                .unwrap_or(50)
                .clamp(1, 200),
        )?,
        "correlation" => correlation(
            &dataset,
            required_option(options, "column", "correlation")?,
            required_option(options, "with", "correlation")?,
        )?,
        other => bail!("unsupported data analysis operation: {other}"),
    };
    if let Value::Object(object) = &mut result {
        object.insert("sourceKind".into(), json!(dataset.source_kind));
        object.insert("sheet".into(), json!(dataset.sheet));
        object.insert("rowCount".into(), json!(dataset.rows.len()));
        object.insert("columnCount".into(), json!(dataset.columns.len()));
    }
    Ok(result)
}

fn read_plain_document(path: &Path, extension: &str) -> Result<DocumentContent> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("failed to read {} as UTF-8", path.display()))?;
    let media_type = match extension {
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "xml" => "application/xml",
        "yaml" | "yml" => "application/yaml",
        "toml" => "application/toml",
        "rtf" => "application/rtf",
        _ => "text/plain",
    };
    Ok(DocumentContent {
        kind: if matches!(extension, "md" | "markdown") {
            "markdown".into()
        } else {
            "text".into()
        },
        media_type: media_type.into(),
        metadata: json!({"characters": text.chars().count(), "lines": text.lines().count()}),
        text,
    })
}

fn read_docx(path: &Path) -> Result<DocumentContent> {
    let file = File::open(path)?;
    let mut archive = ZipArchive::new(file)
        .with_context(|| format!("failed to open DOCX archive {}", path.display()))?;
    let mut parts = Vec::new();
    for name in [
        "word/document.xml",
        "word/footnotes.xml",
        "word/endnotes.xml",
    ] {
        if let Ok(mut entry) = archive.by_name(name) {
            let mut xml = String::new();
            entry.read_to_string(&mut xml)?;
            let text = extract_wordprocessing_text(&xml)?;
            if !text.trim().is_empty() {
                parts.push(text);
            }
        }
    }
    let text = parts.join("\n\n");
    if text.trim().is_empty() {
        bail!("DOCX contains no extractable text");
    }
    Ok(DocumentContent {
        kind: "docx".into(),
        media_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            .into(),
        metadata: json!({"characters": text.chars().count(), "lines": text.lines().count()}),
        text,
    })
}

fn extract_wordprocessing_text(xml: &str) -> Result<String> {
    let mut reader = XmlReader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut text = String::new();
    loop {
        match reader.read_event()? {
            Event::Text(value) => text.push_str(unescape(value.as_ref())?.as_ref()),
            Event::GeneralRef(value) => {
                let escaped = format!("&{};", value.as_ref());
                text.push_str(unescape(&escaped)?.as_ref());
            }
            Event::Start(value) | Event::Empty(value) => {
                match local_xml_name(value.name().as_ref()) {
                    "tab" => text.push('\t'),
                    "br" | "cr" => text.push('\n'),
                    _ => {}
                }
            }
            Event::End(value) => match local_xml_name(value.name().as_ref()) {
                "p" | "tr" => push_line_break(&mut text),
                "tc" if !text.ends_with('\t') && !text.ends_with('\n') => text.push('\t'),
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
    }
    Ok(normalize_extracted_text(&text))
}

fn local_xml_name(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

fn push_line_break(text: &mut String) {
    while text.ends_with(' ') || text.ends_with('\t') {
        text.pop();
    }
    if !text.ends_with('\n') {
        text.push('\n');
    }
}

fn normalize_extracted_text(text: &str) -> String {
    let mut out = String::new();
    let mut blank = false;
    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if !blank && !out.is_empty() {
                out.push('\n');
            }
            blank = true;
        } else {
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(line);
            out.push('\n');
            blank = false;
        }
    }
    out.trim().to_owned()
}

fn read_spreadsheet_document(path: &Path) -> Result<DocumentContent> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("failed to open spreadsheet {}", path.display()))?;
    let sheet_names = workbook.sheet_names().to_vec();
    let mut text = String::new();
    let mut rendered_rows = 0usize;
    let mut truncated = false;
    for sheet_name in &sheet_names {
        let range = workbook.worksheet_range(sheet_name)?;
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(&format!("# Sheet: {sheet_name}\n"));
        for (row_index, row) in range.rows().enumerate() {
            if row_index >= SPREADSHEET_DOCUMENT_ROWS {
                truncated = true;
                text.push_str("[additional rows omitted]\n");
                break;
            }
            let values = row
                .iter()
                .take(SPREADSHEET_DOCUMENT_COLUMNS)
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            if row.len() > SPREADSHEET_DOCUMENT_COLUMNS {
                truncated = true;
            }
            text.push_str(&values.join("\t"));
            text.push('\n');
            rendered_rows += 1;
        }
    }
    Ok(DocumentContent {
        kind: "spreadsheet".into(),
        media_type: spreadsheet_media_type(&extension(path)).into(),
        metadata: json!({"sheets": sheet_names, "renderedRows": rendered_rows, "truncated": truncated}),
        text: text.trim().to_owned(),
    })
}

fn read_delimited_document(path: &Path, tsv: bool) -> Result<DocumentContent> {
    let dataset = load_delimited(path, if tsv { b'\t' } else { b',' })?;
    let mut text = dataset.columns.join("\t");
    text.push('\n');
    let mut truncated = false;
    for row in dataset.rows.iter().take(SPREADSHEET_DOCUMENT_ROWS) {
        let values = row
            .iter()
            .take(SPREADSHEET_DOCUMENT_COLUMNS)
            .map(|value| value.as_deref().unwrap_or(""))
            .collect::<Vec<_>>();
        text.push_str(&values.join("\t"));
        text.push('\n');
    }
    if dataset.rows.len() > SPREADSHEET_DOCUMENT_ROWS
        || dataset.columns.len() > SPREADSHEET_DOCUMENT_COLUMNS
    {
        truncated = true;
        text.push_str("[additional data omitted]\n");
    }
    Ok(DocumentContent {
        kind: "delimited-data".into(),
        media_type: if tsv {
            "text/tab-separated-values"
        } else {
            "text/csv"
        }
        .into(),
        metadata: json!({"rows": dataset.rows.len(), "columns": dataset.columns, "truncated": truncated}),
        text: text.trim().to_owned(),
    })
}

fn load_dataset(path: &Path, sheet: Option<&str>) -> Result<DataSet> {
    if !path.is_file() {
        bail!("data path is not a file: {}", path.display());
    }
    match extension(path).as_str() {
        "csv" => load_delimited(path, b','),
        "tsv" => load_delimited(path, b'\t'),
        "json" => load_json(path),
        "xlsx" | "xls" | "xlsb" | "ods" => load_spreadsheet(path, sheet),
        other => bail!(
            "unsupported data format .{other}; analyze_data supports CSV, TSV, JSON arrays of objects, XLS/XLSX/XLSB, and ODS"
        ),
    }
}

fn load_delimited(path: &Path, delimiter: u8) -> Result<DataSet> {
    let mut reader = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .flexible(true)
        .from_path(path)?;
    let headers = make_unique_headers(
        reader
            .headers()?
            .iter()
            .enumerate()
            .map(|(index, value)| header_or_default(value, index))
            .collect(),
    );
    validate_column_count(headers.len())?;
    let mut rows = Vec::new();
    for record in reader.records() {
        if rows.len() >= MAX_DATA_ROWS {
            bail!("dataset exceeds the built-in analysis limit of {MAX_DATA_ROWS} rows");
        }
        let record = record?;
        let mut row = Vec::with_capacity(headers.len());
        for index in 0..headers.len() {
            row.push(record.get(index).and_then(non_empty_string));
        }
        rows.push(row);
    }
    Ok(DataSet {
        source_kind: if delimiter == b'\t' { "tsv" } else { "csv" }.into(),
        sheet: None,
        columns: headers,
        rows,
    })
}

fn load_json(path: &Path) -> Result<DataSet> {
    let size = fs::metadata(path)?.len();
    if size > MAX_JSON_BYTES {
        bail!("JSON file is larger than the built-in analysis limit of {MAX_JSON_BYTES} bytes");
    }
    let value: Value = serde_json::from_slice(&fs::read(path)?)?;
    let values = value
        .as_array()
        .ok_or_else(|| anyhow!("JSON data analysis expects a top-level array of objects"))?;
    if values.len() > MAX_DATA_ROWS {
        bail!("dataset exceeds the built-in analysis limit of {MAX_DATA_ROWS} rows");
    }
    let mut columns = Vec::<String>::new();
    let mut seen = HashMap::<String, usize>::new();
    for value in values {
        let object = value.as_object().ok_or_else(|| {
            anyhow!("JSON data analysis expects every array item to be an object")
        })?;
        for key in object.keys() {
            if !seen.contains_key(key) {
                seen.insert(key.clone(), columns.len());
                columns.push(key.clone());
            }
        }
    }
    validate_column_count(columns.len())?;
    let rows = values
        .iter()
        .map(|value| {
            let object = value.as_object().expect("validated above");
            columns
                .iter()
                .map(|column| object.get(column).and_then(json_cell_string))
                .collect::<Vec<_>>()
        })
        .collect();
    Ok(DataSet {
        source_kind: "json".into(),
        sheet: None,
        columns,
        rows,
    })
}

fn load_spreadsheet(path: &Path, requested_sheet: Option<&str>) -> Result<DataSet> {
    let mut workbook = open_workbook_auto(path)
        .with_context(|| format!("failed to open spreadsheet {}", path.display()))?;
    let names = workbook.sheet_names().to_vec();
    let sheet = requested_sheet
        .map(str::to_owned)
        .or_else(|| names.first().cloned())
        .ok_or_else(|| anyhow!("spreadsheet has no worksheets"))?;
    if !names.iter().any(|name| name == &sheet) {
        bail!(
            "worksheet {sheet:?} does not exist; available sheets: {}",
            names.join(", ")
        );
    }
    let range = workbook.worksheet_range(&sheet)?;
    let mut iter = range.rows();
    let first = iter.next().unwrap_or(&[]);
    let width = range.width();
    validate_column_count(width)?;
    let columns = make_unique_headers(
        (0..width)
            .map(|index| {
                first
                    .get(index)
                    .map(ToString::to_string)
                    .map(|value| header_or_default(&value, index))
                    .unwrap_or_else(|| default_column_name(index))
            })
            .collect(),
    );
    let mut rows = Vec::new();
    for row in iter {
        if rows.len() >= MAX_DATA_ROWS {
            bail!("dataset exceeds the built-in analysis limit of {MAX_DATA_ROWS} rows");
        }
        rows.push(
            (0..width)
                .map(|index| {
                    row.get(index)
                        .map(ToString::to_string)
                        .and_then(|value| non_empty_string(&value))
                })
                .collect(),
        );
    }
    Ok(DataSet {
        source_kind: extension(path),
        sheet: Some(sheet),
        columns,
        rows,
    })
}

fn describe_dataset(dataset: &DataSet) -> Value {
    let columns = dataset
        .columns
        .iter()
        .enumerate()
        .map(|(index, name)| describe_column(dataset, index, name))
        .collect::<Vec<_>>();
    let sample = dataset
        .rows
        .iter()
        .take(8)
        .map(|row| row_to_json(dataset, row))
        .collect::<Vec<_>>();
    json!({"operation":"describe","columns":columns,"sampleRows":sample})
}

fn describe_column(dataset: &DataSet, index: usize, name: &str) -> Value {
    let mut counts = HashMap::<String, usize>::new();
    let mut missing = 0usize;
    let mut numeric = 0usize;
    let mut boolean = 0usize;
    let mut sum = 0f64;
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut mean = 0f64;
    let mut m2 = 0f64;
    for row in &dataset.rows {
        let Some(value) = row.get(index).and_then(Option::as_deref) else {
            missing += 1;
            continue;
        };
        *counts.entry(value.to_owned()).or_default() += 1;
        if let Ok(number) = value.parse::<f64>() {
            numeric += 1;
            sum += number;
            min = min.min(number);
            max = max.max(number);
            let delta = number - mean;
            mean += delta / numeric as f64;
            m2 += delta * (number - mean);
        }
        if matches!(value.to_ascii_lowercase().as_str(), "true" | "false") {
            boolean += 1;
        }
    }
    let present = dataset.rows.len().saturating_sub(missing);
    let inferred_type = if present == 0 {
        "empty"
    } else if numeric == present {
        "number"
    } else if boolean == present {
        "boolean"
    } else if numeric > 0 || boolean > 0 {
        "mixed"
    } else {
        "text"
    };
    let mut top = counts.into_iter().collect::<Vec<_>>();
    top.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    top.truncate(5);
    let numeric_stats = if numeric > 0 {
        Some(json!({
            "count": numeric,
            "sum": sum,
            "mean": mean,
            "min": min,
            "max": max,
            "stdDev": if numeric > 1 { (m2 / (numeric as f64 - 1.0)).sqrt() } else { 0.0 }
        }))
    } else {
        None
    };
    json!({
        "name": name,
        "type": inferred_type,
        "missing": missing,
        "present": present,
        "unique": top_unique_count(dataset, index),
        "topValues": top.into_iter().map(|(value,count)| json!({"value":value,"count":count})).collect::<Vec<_>>(),
        "numeric": numeric_stats,
    })
}

fn top_unique_count(dataset: &DataSet, index: usize) -> usize {
    let mut values = std::collections::HashSet::new();
    for row in &dataset.rows {
        if let Some(value) = row.get(index).and_then(Option::as_deref) {
            values.insert(value);
        }
    }
    values.len()
}

fn value_counts(dataset: &DataSet, column: &str, max_groups: usize) -> Result<Value> {
    let index = column_index(dataset, column)?;
    let mut counts = HashMap::<String, usize>::new();
    let mut missing = 0usize;
    for row in &dataset.rows {
        if let Some(value) = row.get(index).and_then(Option::as_deref) {
            *counts.entry(value.to_owned()).or_default() += 1;
        } else {
            missing += 1;
        }
    }
    let mut values = counts.into_iter().collect::<Vec<_>>();
    values.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total_groups = values.len();
    values.truncate(max_groups);
    Ok(json!({
        "operation":"value_counts",
        "column":column,
        "missing":missing,
        "totalGroups":total_groups,
        "groups":values.into_iter().map(|(value,count)| json!({"value":value,"count":count})).collect::<Vec<_>>()
    }))
}

#[derive(Debug, Default, Clone)]
struct Aggregate {
    count: usize,
    numeric_count: usize,
    sum: f64,
    min: f64,
    max: f64,
}

fn group_by(
    dataset: &DataSet,
    group_column: &str,
    value_column: Option<&str>,
    aggregation: &str,
    max_groups: usize,
) -> Result<Value> {
    if !matches!(aggregation, "count" | "sum" | "mean" | "min" | "max") {
        bail!("unsupported group_by aggregation: {aggregation}");
    }
    let group_index = column_index(dataset, group_column)?;
    let value_index = match aggregation {
        "count" => value_column
            .map(|value| column_index(dataset, value))
            .transpose()?,
        _ => Some(column_index(
            dataset,
            value_column.ok_or_else(|| {
                anyhow!("group_by aggregation {aggregation} requires valueColumn")
            })?,
        )?),
    };
    let mut groups = BTreeMap::<String, Aggregate>::new();
    for row in &dataset.rows {
        let key = row
            .get(group_index)
            .and_then(Option::as_deref)
            .unwrap_or("(missing)")
            .to_owned();
        let entry = groups.entry(key).or_insert_with(|| Aggregate {
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
            ..Aggregate::default()
        });
        entry.count += 1;
        if let Some(index) = value_index
            && let Some(number) = row
                .get(index)
                .and_then(Option::as_deref)
                .and_then(|value| value.parse::<f64>().ok())
        {
            entry.numeric_count += 1;
            entry.sum += number;
            entry.min = entry.min.min(number);
            entry.max = entry.max.max(number);
        }
    }
    let total_groups = groups.len();
    let mut values = groups
        .into_iter()
        .map(|(group, aggregate)| {
            let value = match aggregation {
                "count" => aggregate.count as f64,
                "sum" => aggregate.sum,
                "mean" => {
                    if aggregate.numeric_count == 0 {
                        f64::NAN
                    } else {
                        aggregate.sum / aggregate.numeric_count as f64
                    }
                }
                "min" => aggregate.min,
                "max" => aggregate.max,
                _ => unreachable!(),
            };
            (group, aggregate, value)
        })
        .collect::<Vec<_>>();
    values.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    values.truncate(max_groups);
    Ok(json!({
        "operation":"group_by",
        "groupBy":group_column,
        "valueColumn":value_column,
        "aggregation":aggregation,
        "totalGroups":total_groups,
        "groups":values.into_iter().map(|(group, aggregate, value)| json!({
            "group":group,
            "rows":aggregate.count,
            "numericValues":aggregate.numeric_count,
            "value": if value.is_finite() { Value::from(value) } else { Value::Null }
        })).collect::<Vec<_>>()
    }))
}

fn correlation(dataset: &DataSet, left: &str, right: &str) -> Result<Value> {
    let left_index = column_index(dataset, left)?;
    let right_index = column_index(dataset, right)?;
    let mut pairs = Vec::new();
    for row in &dataset.rows {
        let a = row
            .get(left_index)
            .and_then(Option::as_deref)
            .and_then(|value| value.parse::<f64>().ok());
        let b = row
            .get(right_index)
            .and_then(Option::as_deref)
            .and_then(|value| value.parse::<f64>().ok());
        if let (Some(a), Some(b)) = (a, b) {
            pairs.push((a, b));
        }
    }
    if pairs.len() < 2 {
        bail!("correlation requires at least two rows where both columns are numeric");
    }
    let n = pairs.len() as f64;
    let mean_a = pairs.iter().map(|(a, _)| a).sum::<f64>() / n;
    let mean_b = pairs.iter().map(|(_, b)| b).sum::<f64>() / n;
    let mut covariance = 0f64;
    let mut variance_a = 0f64;
    let mut variance_b = 0f64;
    for (a, b) in &pairs {
        let da = a - mean_a;
        let db = b - mean_b;
        covariance += da * db;
        variance_a += da * da;
        variance_b += db * db;
    }
    let denominator = (variance_a * variance_b).sqrt();
    if denominator == 0.0 {
        bail!("correlation is undefined because one selected column has zero variance");
    }
    Ok(json!({
        "operation":"correlation",
        "column":left,
        "with":right,
        "pairs":pairs.len(),
        "pearson":covariance / denominator
    }))
}

fn row_to_json(dataset: &DataSet, row: &[Option<String>]) -> Value {
    let mut object = Map::new();
    for (index, column) in dataset.columns.iter().enumerate() {
        object.insert(
            column.clone(),
            row.get(index)
                .and_then(Option::as_ref)
                .map(|value| Value::String(value.clone()))
                .unwrap_or(Value::Null),
        );
    }
    Value::Object(object)
}

fn column_index(dataset: &DataSet, column: &str) -> Result<usize> {
    dataset
        .columns
        .iter()
        .position(|value| value == column)
        .ok_or_else(|| {
            anyhow!(
                "unknown column {column:?}; available columns: {}",
                dataset.columns.join(", ")
            )
        })
}

fn required_option<'a>(
    options: &'a Map<String, Value>,
    key: &str,
    operation: &str,
) -> Result<&'a str> {
    options
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow!("{operation} requires {key}"))
}

fn usize_option(options: &Map<String, Value>, key: &str) -> Option<usize> {
    options
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| usize::try_from(value).ok())
}

fn json_cell_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => non_empty_string(value),
        Value::Bool(value) => Some(value.to_string()),
        Value::Number(value) => Some(value.to_string()),
        other => Some(other.to_string()),
    }
}

fn non_empty_string(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

fn validate_column_count(columns: usize) -> Result<()> {
    if columns == 0 {
        bail!("dataset has no columns");
    }
    if columns > MAX_DATA_COLUMNS {
        bail!("dataset exceeds the built-in analysis limit of {MAX_DATA_COLUMNS} columns");
    }
    Ok(())
}

fn make_unique_headers(headers: Vec<String>) -> Vec<String> {
    let mut counts = HashMap::<String, usize>::new();
    headers
        .into_iter()
        .map(|header| {
            let count = counts.entry(header.clone()).or_default();
            *count += 1;
            if *count == 1 {
                header
            } else {
                format!("{header}_{}", *count)
            }
        })
        .collect()
}

fn header_or_default(value: &str, index: usize) -> String {
    let value = value.trim();
    if value.is_empty() {
        default_column_name(index)
    } else {
        value.to_owned()
    }
}

fn default_column_name(index: usize) -> String {
    format!("column_{}", index + 1)
}

fn extension(path: &Path) -> String {
    path.extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn spreadsheet_media_type(extension: &str) -> &'static str {
    match extension {
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "xls" => "application/vnd.ms-excel",
        "ods" => "application/vnd.oasis.opendocument.spreadsheet",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wordprocessing_xml_extracts_paragraphs_tabs_and_entities() {
        let xml = r#"<w:document xmlns:w="urn:w"><w:body><w:p><w:r><w:t>Hello &amp; world</w:t></w:r></w:p><w:p><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t></w:r></w:p></w:body></w:document>"#;
        let text = extract_wordprocessing_text(xml).unwrap();
        assert_eq!(text, "Hello & world\nA\tB");
    }

    #[test]
    fn csv_analysis_describes_groups_and_correlation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.csv");
        fs::write(&path, "team,x,y\nred,1,2\nred,2,4\nblue,3,6\nblue,,8\n").unwrap();

        let describe =
            analyze_data(&path, json!({"operation":"describe"}).as_object().unwrap()).unwrap();
        assert_eq!(describe["rowCount"], 4);
        assert_eq!(describe["columns"][1]["numeric"]["mean"], 2.0);

        let grouped = analyze_data(
            &path,
            json!({"operation":"group_by","groupBy":"team","valueColumn":"x","aggregation":"mean"})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(grouped["totalGroups"], 2);

        let correlation = analyze_data(
            &path,
            json!({"operation":"correlation","column":"x","with":"y"})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(correlation["pairs"], 3);
        assert!((correlation["pearson"].as_f64().unwrap() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn json_analysis_unions_object_columns() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("data.json");
        fs::write(&path, r#"[{"a":1,"name":"x"},{"a":2,"b":true}]"#).unwrap();

        let result = analyze_data(&path, json!({}).as_object().unwrap()).unwrap();
        assert_eq!(result["rowCount"], 2);
        let names = result["columns"]
            .as_array()
            .unwrap()
            .iter()
            .map(|column| column["name"].as_str().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["a", "name", "b"]);
    }
}
