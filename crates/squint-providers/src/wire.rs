//! Wire schemas (SPEC "Wire schemas", "Submission schema"). Each adapter derives the schema it sends
//! for `submit_verdict` and for every tool from the canonical schema by the versioned transformation
//! here. Constraints a provider cannot carry are removed and stay enforced by canonical validation,
//! so a wire schema is only ever looser than the canonical one; constructs a provider cannot
//! represent at all are refused with the reason at resolve time, before any billable request.
//! `normalize` takes a wire payload back to the canonical shape before validation.
//!
//! The provider rules were read from the providers' documentation on 2026-10-26 (sources in
//! `crates/squint-providers/catalog.toml`); the catalog's `strict_unsupported_keywords` names the constraint
//! keywords each model's strict mode rejects, and the structural rules live in `Rules` below.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use squint_core::provider::ToolDefinition;
use squint_core::run::SUBMIT;
use squint_core::{Error, Result};

use crate::catalog::Capabilities;

/// The transformation version; part of every wire schema and of the cache key through the catalog.
pub const WIRE_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Adapter {
    AnthropicMessages,
    GeminiGenerateContent,
    OpenaiResponses,
}

impl Adapter {
    pub fn parse(id: &str) -> Option<Adapter> {
        match id {
            "anthropic-messages" => Some(Adapter::AnthropicMessages),
            "gemini-generate-content" => Some(Adapter::GeminiGenerateContent),
            "openai-responses" => Some(Adapter::OpenaiResponses),
            _ => None,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Adapter::AnthropicMessages => "anthropic-messages",
            Adapter::GeminiGenerateContent => "gemini-generate-content",
            Adapter::OpenaiResponses => "openai-responses",
        }
    }
}

/// A transformed schema with the record of what changed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireSchema {
    pub adapter: Adapter,
    pub wire_version: u32,
    /// Whether the adapter asks the provider to enforce this schema (`strict: true`).
    pub strict: bool,
    pub schema: Value,
    /// Every removal and rewrite as `<json pointer>: <what>`, in document order.
    pub changes: Vec<String>,
}

/// A tool as the provider will see it, with the transformation record.
#[derive(Debug, Clone, PartialEq)]
pub struct WireTool {
    pub definition: ToolDefinition,
    pub wire: WireSchema,
}

/// Per-adapter structural rules. Constraint removals come from the catalog.
struct Rules {
    /// String formats the provider accepts; any other `format` is removed.
    string_formats: &'static [&'static str],
    /// The largest `minItems` kept (Anthropic strict mode accepts only 0 and 1).
    min_items_max: Option<u64>,
    const_supported: bool,
    default_supported: bool,
    prefix_items_supported: bool,
    /// Replace `$ref` by the definition and drop `$defs` (Gemini's documentation does not state
    /// that `$defs` is accepted in `parametersJsonSchema`).
    inline_refs: bool,
    recursion_allowed: bool,
    /// Every object closed: `additionalProperties: false` set where absent or `true`.
    close_objects: bool,
    /// Every property required, optional ones made nullable (OpenAI strict mode).
    require_all: bool,
    /// A `$ref` node carries no other keyword (OpenAI strict mode rejects a sibling description).
    bare_refs: bool,
    /// A nullable array or object is written as `anyOf` with `null` rather than a type array
    /// (OpenAI strict mode accepts type arrays for primitives only).
    nullable_structural_as_union: bool,
    strict: bool,
}

const ANTHROPIC_FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "uri",
    "ipv4",
    "ipv6",
    "uuid",
];
const OPENAI_FORMATS: &[&str] = &[
    "date-time",
    "time",
    "date",
    "duration",
    "email",
    "hostname",
    "ipv4",
    "ipv6",
    "uuid",
];
const GEMINI_FORMATS: &[&str] = &["date-time", "date", "time"];

fn rules(adapter: Adapter, caps: &Capabilities) -> Option<Rules> {
    match adapter {
        // Without strict mode Anthropic takes the canonical schema as it is.
        Adapter::AnthropicMessages if !caps.strict_tools => None,
        Adapter::AnthropicMessages => Some(Rules {
            string_formats: ANTHROPIC_FORMATS,
            min_items_max: Some(1),
            const_supported: true,
            default_supported: true,
            prefix_items_supported: false,
            inline_refs: false,
            recursion_allowed: false,
            close_objects: true,
            require_all: false,
            bare_refs: false,
            nullable_structural_as_union: false,
            strict: true,
        }),
        Adapter::GeminiGenerateContent => Some(Rules {
            string_formats: GEMINI_FORMATS,
            min_items_max: None,
            const_supported: false,
            default_supported: true,
            prefix_items_supported: true,
            inline_refs: true,
            recursion_allowed: false,
            close_objects: false,
            require_all: false,
            bare_refs: false,
            nullable_structural_as_union: false,
            strict: false,
        }),
        Adapter::OpenaiResponses => Some(Rules {
            string_formats: OPENAI_FORMATS,
            min_items_max: None,
            const_supported: false,
            default_supported: false,
            prefix_items_supported: false,
            inline_refs: false,
            recursion_allowed: true,
            close_objects: true,
            require_all: true,
            bare_refs: true,
            nullable_structural_as_union: true,
            strict: caps.strict_tools,
        }),
    }
}

/// Keywords every wire subset knows. Anything else is refused as not representable.
const KNOWN: &[&str] = &[
    "type",
    "properties",
    "required",
    "additionalProperties",
    "items",
    "prefixItems",
    "enum",
    "const",
    "anyOf",
    "oneOf",
    "$ref",
    "$defs",
    "definitions",
    "description",
    "title",
    "default",
    "pattern",
    "format",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minProperties",
    "maxProperties",
    "contains",
    "minContains",
    "maxContains",
    "$schema",
    "examples",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$id",
];
/// Annotations and metadata no provider needs; removed silently.
const META: &[&str] = &[
    "$schema",
    "title",
    "examples",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
    "$id",
];
/// Constructs no adapter represents; a schema carrying one is refused.
const REFUSED: &[&str] = &[
    "allOf",
    "not",
    "if",
    "then",
    "else",
    "dependentRequired",
    "dependentSchemas",
    "patternProperties",
    "propertyNames",
    "unevaluatedProperties",
    "unevaluatedItems",
    "$dynamicRef",
    "$dynamicAnchor",
    "$anchor",
];

/// Transform one canonical schema for the adapter. The name is for messages only.
pub fn transform(
    adapter: Adapter,
    caps: &Capabilities,
    name: &str,
    canonical: &Value,
) -> Result<WireSchema> {
    let Some(rules) = rules(adapter, caps) else {
        return Ok(WireSchema {
            adapter,
            wire_version: WIRE_VERSION,
            strict: false,
            schema: canonical.clone(),
            changes: Vec::new(),
        });
    };
    let fail = |what: String| {
        Error::Config(format!(
            "wire ({}): schema of {name:?} cannot be represented: {what}",
            adapter.id()
        ))
    };
    if canonical.get("type") != Some(&Value::String("object".into())) {
        return Err(fail("the root must be an object".into()));
    }
    if canonical.get("anyOf").is_some() || canonical.get("oneOf").is_some() {
        return Err(fail("the root must not be a union".into()));
    }
    let defs = definitions(canonical);
    if let Some(cycle) = find_cycle(canonical, &defs)
        && !rules.recursion_allowed
    {
        return Err(fail(format!("recursive definition {cycle:?}")));
    }
    let mut t = Transformer {
        rules: &rules,
        removed: &caps.strict_unsupported_keywords,
        changes: Vec::new(),
        refusal: None,
    };
    let mut schema = canonical.clone();
    t.walk("", &mut schema);
    if let Some(reason) = t.refusal {
        return Err(fail(reason));
    }
    if rules.inline_refs {
        let defs = definitions(&schema);
        inline(&mut schema, &defs);
        if let Some(o) = schema.as_object_mut() {
            let had_defs = o.remove("$defs").is_some();
            let had_definitions = o.remove("definitions").is_some();
            if had_defs || had_definitions {
                t.changes
                    .push("/$defs: inlined into every reference".into());
            }
        }
    }
    Ok(WireSchema {
        adapter,
        wire_version: WIRE_VERSION,
        strict: rules.strict,
        schema,
        changes: t.changes,
    })
}

/// Transform a tool's model schema.
pub fn transform_tool(
    adapter: Adapter,
    caps: &Capabilities,
    def: &ToolDefinition,
) -> Result<WireTool> {
    let wire = transform(adapter, caps, &def.name, &def.input_schema)?;
    Ok(WireTool {
        definition: ToolDefinition {
            name: def.name.clone(),
            description: def.description.clone(),
            input_schema: wire.schema.clone(),
        },
        wire,
    })
}

/// The resolve-time check: every definition (the enabled tools and `submit_verdict`) must pass the
/// transformation, and the set must fit the provider's complexity limits.
pub fn check_tool_set(
    adapter: Adapter,
    caps: &Capabilities,
    defs: &[ToolDefinition],
) -> Result<Vec<WireTool>> {
    // Anthropic compiles every strict schema in a request into one grammar and refuses the request
    // when that grammar is too large ("The compiled grammar is too large…"), a limit its guide does
    // not state as a number. `submit_verdict` alone already fills most of it, so on Anthropic only
    // `submit_verdict` is strict: the inspection tools go as their canonical schemas, and their
    // arguments stay enforced by canonical validation, an invalid call answered as `invalid_args`.
    let non_strict = Capabilities {
        strict_tools: false,
        ..caps.clone()
    };
    let tools: Vec<WireTool> = defs
        .iter()
        .map(|d| match adapter {
            Adapter::AnthropicMessages if d.name != SUBMIT => {
                transform_tool(adapter, &non_strict, d)
            }
            _ => transform_tool(adapter, caps, d),
        })
        .collect::<Result<_>>()?;
    let fail = |what: String| Error::Config(format!("wire ({}): {what}", adapter.id()));
    match adapter {
        Adapter::AnthropicMessages if caps.strict_tools => {
            // Limits read from the structured-outputs guide: 20 strict tools, 24 optional
            // parameters and 16 union-typed parameters across every strict schema in a request.
            let strict: Vec<&WireTool> = tools.iter().filter(|t| t.wire.strict).collect();
            if strict.len() > 20 {
                return Err(fail(format!(
                    "{} tools exceed the 20 strict tools a request may carry",
                    strict.len()
                )));
            }
            let mut optional = 0usize;
            let mut unions = 0usize;
            for t in strict {
                let s = expanded(&t.wire.schema);
                let c = Counts::of(&s);
                optional += c.optional;
                unions += c.unions;
            }
            if optional > 24 {
                return Err(fail(format!(
                    "{optional} optional parameters across the tool set exceed the strict-mode limit of 24"
                )));
            }
            if unions > 16 {
                return Err(fail(format!(
                    "{unions} union-typed parameters across the tool set exceed the strict-mode limit of 16"
                )));
            }
        }
        Adapter::OpenaiResponses => {
            // Limits read from the structured-outputs guide: 5,000 properties, 10 levels of
            // nesting, 1,000 enum values, 120,000 characters of names and values per schema.
            for t in &tools {
                let s = expanded(&t.wire.schema);
                let c = Counts::of(&s);
                let name = &t.definition.name;
                if c.properties > 5000 {
                    return Err(fail(format!(
                        "{name:?}: {} properties exceed the limit of 5,000",
                        c.properties
                    )));
                }
                if c.depth > 10 {
                    return Err(fail(format!(
                        "{name:?}: nesting depth {} exceeds the limit of 10",
                        c.depth
                    )));
                }
                if c.enum_values > 1000 {
                    return Err(fail(format!(
                        "{name:?}: {} enum values exceed the limit of 1,000",
                        c.enum_values
                    )));
                }
                if c.chars > 120_000 {
                    return Err(fail(format!(
                        "{name:?}: {} characters of names and values exceed the limit of 120,000",
                        c.chars
                    )));
                }
            }
        }
        _ => {}
    }
    Ok(tools)
}

/// A wire payload in canonical form: `null` dropped from properties the canonical schema leaves
/// optional (OpenAI's nullable emulation), `null` supplied for required nullable properties a
/// provider left out (Gemini omits null-valued fields), and enum strings restored to the canonical
/// spelling (Anthropic does not guarantee the capitalisation of enum values).
pub fn normalize(canonical: &Value, payload: Value) -> Value {
    let defs = definitions(canonical);
    let mut v = payload;
    normalize_at(canonical, &defs, &mut v, 0);
    v
}

fn normalize_at(schema: &Value, defs: &BTreeMap<String, Value>, v: &mut Value, depth: usize) {
    if depth > 64 {
        return;
    }
    let schema = resolve(schema, defs);
    if let Some(alts) = schema
        .get("anyOf")
        .or(schema.get("oneOf"))
        .and_then(Value::as_array)
    {
        if let Some(alt) = alts.iter().find(|a| matches(resolve(a, defs), v)) {
            normalize_at(alt, defs, v, depth + 1);
        }
        return;
    }
    match v {
        Value::Object(o) => {
            let required: BTreeSet<&str> = schema
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if let Some(props) = schema.get("properties").and_then(Value::as_object) {
                o.retain(|k, val| {
                    !(val.is_null() && props.contains_key(k) && !required.contains(k.as_str()))
                });
                for (k, p) in props {
                    if required.contains(k.as_str())
                        && !o.contains_key(k)
                        && accepts_null(resolve(p, defs), defs)
                    {
                        o.insert(k.clone(), Value::Null);
                    }
                }
                for (k, val) in o.iter_mut() {
                    if let Some(p) = props.get(k) {
                        normalize_at(p, defs, val, depth + 1);
                    }
                }
            }
        }
        Value::Array(a) => {
            if let Some(items) = schema.get("items") {
                for item in a.iter_mut() {
                    normalize_at(items, defs, item, depth + 1);
                }
            }
        }
        Value::String(s) => {
            let spelled = schema
                .get("enum")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .chain(schema.get("const"))
                .filter_map(Value::as_str)
                .find(|e| e.eq_ignore_ascii_case(s) && *e != s.as_str());
            if let Some(e) = spelled {
                *s = e.to_string();
            }
        }
        _ => {}
    }
}

/// Whether a schema admits `null`: a `null` type, a type array holding it, or a union alternative.
fn accepts_null(schema: &Value, defs: &BTreeMap<String, Value>) -> bool {
    match schema.get("type") {
        Some(Value::String(t)) if t == "null" => return true,
        Some(Value::Array(a)) if a.iter().any(|t| t == "null") => return true,
        _ => {}
    }
    schema
        .get("anyOf")
        .or(schema.get("oneOf"))
        .and_then(Value::as_array)
        .is_some_and(|alts| alts.iter().any(|a| accepts_null(resolve(a, defs), defs)))
}

/// Whether a payload value plausibly belongs to an alternative: by declared type, or by an enum.
fn matches(schema: &Value, v: &Value) -> bool {
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let kind = match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    };
    if !types.is_empty() {
        return types.contains(&kind) || (kind == "integer" && types.contains(&"number"));
    }
    if let Some(e) = schema.get("enum").and_then(Value::as_array) {
        return e.iter().any(|x| x == v || matches!((x, v), (Value::String(a), Value::String(b)) if a.eq_ignore_ascii_case(b)));
    }
    if schema.get("const").is_some() {
        return schema.get("const") == Some(v);
    }
    schema.get("properties").is_some() && v.is_object()
}

fn resolve<'a>(schema: &'a Value, defs: &'a BTreeMap<String, Value>) -> &'a Value {
    match schema
        .get("$ref")
        .and_then(Value::as_str)
        .and_then(def_name)
    {
        Some(name) => defs.get(name).unwrap_or(schema),
        None => schema,
    }
}

fn def_name(r: &str) -> Option<&str> {
    r.strip_prefix("#/$defs/")
        .or_else(|| r.strip_prefix("#/definitions/"))
}

fn definitions(schema: &Value) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for key in ["$defs", "definitions"] {
        if let Some(o) = schema.get(key).and_then(Value::as_object) {
            for (k, v) in o {
                out.insert(k.clone(), v.clone());
            }
        }
    }
    out
}

/// The name of a definition that reaches itself, if any (the root counts as `#`).
fn find_cycle(root: &Value, defs: &BTreeMap<String, Value>) -> Option<String> {
    fn refs(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                if let Some(r) = o.get("$ref").and_then(Value::as_str) {
                    out.push(r.to_string());
                }
                for (k, val) in o {
                    if k != "$defs" && k != "definitions" {
                        refs(val, out);
                    }
                }
            }
            Value::Array(a) => a.iter().for_each(|x| refs(x, out)),
            _ => {}
        }
    }
    fn visit(
        name: &str,
        v: &Value,
        defs: &BTreeMap<String, Value>,
        stack: &mut Vec<String>,
    ) -> Option<String> {
        if stack.iter().any(|s| s == name) {
            return Some(name.to_string());
        }
        stack.push(name.to_string());
        let mut rs = Vec::new();
        refs(v, &mut rs);
        for r in rs {
            let next = if r == "#" {
                Some(("#", None))
            } else {
                def_name(&r).map(|n| (n, defs.get(n)))
            };
            match next {
                Some(("#", _)) => return Some("#".into()),
                Some((n, Some(d))) => {
                    if let Some(c) = visit(n, d, defs, stack) {
                        return Some(c);
                    }
                }
                _ => {}
            }
        }
        stack.pop();
        None
    }
    visit("#", root, defs, &mut Vec::new())
}

/// Replace every internal `$ref` by its definition (no cycles by the time this runs).
fn inline(v: &mut Value, defs: &BTreeMap<String, Value>) {
    match v {
        Value::Object(o) => {
            if let Some(name) = o
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(def_name)
                .map(str::to_string)
                && let Some(def) = defs.get(&name)
            {
                let description = o.get("description").cloned();
                let mut replacement = def.clone();
                if let (Some(d), Some(r)) = (description, replacement.as_object_mut()) {
                    r.entry("description").or_insert(d);
                }
                *v = replacement;
                inline(v, defs);
                return;
            }
            for (k, val) in o.iter_mut() {
                if k != "$defs" && k != "definitions" {
                    inline(val, defs);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(|x| inline(x, defs)),
        _ => {}
    }
}

/// A copy with references inlined, for counting against limits.
fn expanded(schema: &Value) -> Value {
    let defs = definitions(schema);
    let mut s = schema.clone();
    if find_cycle(schema, &defs).is_none() {
        inline(&mut s, &defs);
        if let Some(o) = s.as_object_mut() {
            o.remove("$defs");
            o.remove("definitions");
        }
    }
    s
}

#[derive(Default)]
struct Counts {
    properties: usize,
    optional: usize,
    unions: usize,
    enum_values: usize,
    chars: usize,
    depth: usize,
}

impl Counts {
    fn of(schema: &Value) -> Counts {
        let mut c = Counts::default();
        c.visit(schema, 0);
        c
    }

    fn visit(&mut self, v: &Value, depth: usize) {
        let Some(o) = v.as_object() else { return };
        let nested = o.contains_key("properties")
            || o.contains_key("items")
            || o.contains_key("prefixItems");
        if nested {
            self.depth = self.depth.max(depth + 1);
        }
        if let Some(e) = o.get("enum").and_then(Value::as_array) {
            self.enum_values += e.len();
            self.chars += e
                .iter()
                .filter_map(Value::as_str)
                .map(str::len)
                .sum::<usize>();
        }
        if let Some(c) = o.get("const").and_then(Value::as_str) {
            self.chars += c.len();
        }
        if let Some(props) = o.get("properties").and_then(Value::as_object) {
            let required: BTreeSet<&str> = o
                .get("required")
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            for (name, p) in props {
                self.properties += 1;
                self.chars += name.len();
                if !required.contains(name.as_str()) {
                    self.optional += 1;
                }
                let union = p.get("anyOf").is_some()
                    || p.get("oneOf").is_some()
                    || p.get("type").is_some_and(Value::is_array);
                if union {
                    self.unions += 1;
                }
                self.visit(p, depth + 1);
            }
        }
        for key in ["items", "additionalProperties"] {
            if let Some(x) = o.get(key) {
                self.visit(x, depth + 1);
            }
        }
        for key in ["anyOf", "oneOf", "prefixItems"] {
            if let Some(a) = o.get(key).and_then(Value::as_array) {
                for x in a {
                    self.visit(x, depth);
                }
            }
        }
        for key in ["$defs", "definitions"] {
            if let Some(d) = o.get(key).and_then(Value::as_object) {
                for (name, x) in d {
                    self.chars += name.len();
                    self.visit(x, depth);
                }
            }
        }
    }
}

struct Transformer<'a> {
    rules: &'a Rules,
    removed: &'a [String],
    changes: Vec<String>,
    refusal: Option<String>,
}

impl Transformer<'_> {
    fn refuse(&mut self, what: String) {
        if self.refusal.is_none() {
            self.refusal = Some(what);
        }
    }

    fn walk(&mut self, path: &str, v: &mut Value) {
        if self.refusal.is_some() {
            return;
        }
        let Some(o) = v.as_object_mut() else { return };
        // Unions first: `oneOf` becomes `anyOf` (the form every provider documents).
        if let Some(alts) = o.remove("oneOf") {
            if o.contains_key("anyOf") {
                self.refuse(format!("{path}: both oneOf and anyOf"));
                return;
            }
            o.insert("anyOf".into(), alts);
            self.changes
                .push(format!("{path}/oneOf: rewritten as anyOf"));
        }
        for key in o.keys().cloned().collect::<Vec<_>>() {
            if REFUSED.contains(&key.as_str()) {
                self.refuse(format!("{path}: keyword {key:?} has no wire form"));
                return;
            }
            if !KNOWN.contains(&key.as_str()) {
                self.refuse(format!("{path}: keyword {key:?} is not in the wire subset"));
                return;
            }
        }
        if let Some(r) = o.get("$ref").and_then(Value::as_str) {
            if r == "#" && !self.rules.recursion_allowed {
                self.refuse(format!("{path}: recursive reference to the root"));
                return;
            }
            if r != "#" && def_name(r).is_none() {
                self.refuse(format!("{path}: external reference {r:?}"));
                return;
            }
        }
        if o.contains_key("prefixItems") && !self.rules.prefix_items_supported {
            self.refuse(format!("{path}: prefixItems is not supported"));
            return;
        }
        // Metadata and the catalog's unsupported constraints.
        for key in META {
            if o.remove(*key).is_some() && *key != "$schema" && *key != "title" {
                self.changes.push(format!("{path}/{key}: removed"));
            }
        }
        for key in self.removed {
            if o.remove(key.as_str()).is_some() {
                self.changes.push(format!("{path}/{key}: removed"));
            }
        }
        if !self.rules.default_supported && o.remove("default").is_some() {
            self.changes.push(format!("{path}/default: removed"));
        }
        if let Some(max) = self.rules.min_items_max
            && o.get("minItems")
                .and_then(Value::as_u64)
                .is_some_and(|n| n > max)
        {
            o.remove("minItems");
            self.changes.push(format!(
                "{path}/minItems: removed (only 0 and 1 are accepted)"
            ));
        }
        if let Some(f) = o.get("format").and_then(Value::as_str).map(str::to_string) {
            let is_string = match o.get("type") {
                Some(Value::String(t)) => t == "string",
                Some(Value::Array(a)) => a.iter().any(|t| t == "string"),
                _ => false,
            };
            if !(is_string && self.rules.string_formats.contains(&f.as_str())) {
                o.remove("format");
                self.changes.push(format!("{path}/format: removed ({f})"));
            }
        }
        if !self.rules.const_supported
            && let Some(c) = o.remove("const")
        {
            o.insert("enum".into(), Value::Array(vec![c]));
            self.changes
                .push(format!("{path}/const: rewritten as enum"));
        }
        // Objects: closed, and in OpenAI strict mode every property required.
        let is_object =
            o.get("type") == Some(&Value::String("object".into())) || o.contains_key("properties");
        if is_object {
            match o.get("additionalProperties") {
                Some(Value::Bool(false)) => {}
                Some(Value::Bool(true)) | None if self.rules.close_objects => {
                    o.insert("additionalProperties".into(), Value::Bool(false));
                    self.changes
                        .push(format!("{path}/additionalProperties: closed"));
                }
                Some(Value::Object(_)) if self.rules.close_objects => {
                    self.refuse(format!(
                        "{path}: additionalProperties as a schema cannot be closed"
                    ));
                    return;
                }
                _ => {}
            }
            if self.rules.require_all
                && let Some(props) = o.get("properties").and_then(Value::as_object).cloned()
            {
                let mut required: Vec<String> = o
                    .get("required")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default();
                let optional: Vec<String> = props
                    .keys()
                    .filter(|k| !required.contains(k))
                    .cloned()
                    .collect();
                if !optional.is_empty() {
                    let props_mut = o.get_mut("properties").and_then(Value::as_object_mut);
                    if let Some(pm) = props_mut {
                        for name in &optional {
                            if let Some(p) = pm.get_mut(name) {
                                make_nullable(p);
                            }
                            required.push(name.clone());
                            self.changes.push(format!(
                                "{path}/properties/{name}: required and nullable (was optional)"
                            ));
                        }
                    }
                    required.sort();
                    o.insert(
                        "required".into(),
                        Value::Array(required.into_iter().map(Value::String).collect()),
                    );
                }
            }
        }
        // Recurse.
        if let Some(props) = o.get_mut("properties").and_then(Value::as_object_mut) {
            for (name, p) in props.iter_mut() {
                self.walk(&format!("{path}/properties/{name}"), p);
            }
        }
        for key in ["items", "additionalProperties"] {
            if let Some(x) = o.get_mut(key)
                && x.is_object()
            {
                self.walk(&format!("{path}/{key}"), x);
            }
        }
        for key in ["anyOf", "prefixItems"] {
            if let Some(a) = o.get_mut(key).and_then(Value::as_array_mut) {
                for (i, x) in a.iter_mut().enumerate() {
                    self.walk(&format!("{path}/{key}/{i}"), x);
                }
            }
        }
        for key in ["$defs", "definitions"] {
            if let Some(d) = o.get_mut(key).and_then(Value::as_object_mut) {
                for (name, x) in d.iter_mut() {
                    self.walk(&format!("{path}/{key}/{name}"), x);
                }
            }
        }
        // A union of plain enums is one enum.
        if let Some(alts) = o.get("anyOf").and_then(Value::as_array)
            && let Some((ty, values)) = enum_union(alts)
        {
            o.remove("anyOf");
            o.insert("type".into(), Value::String(ty));
            o.insert("enum".into(), Value::Array(values));
            self.changes
                .push(format!("{path}/anyOf: enums merged into one enum"));
        }
        // A reference stands alone where the provider allows nothing beside it.
        if self.rules.bare_refs && o.contains_key("$ref") {
            let extra: Vec<String> = o.keys().filter(|k| *k != "$ref").cloned().collect();
            for k in extra {
                o.remove(&k);
                self.changes
                    .push(format!("{path}/{k}: removed (beside $ref)"));
            }
        }
        // A nullable array or object becomes a union with null.
        if self.rules.nullable_structural_as_union
            && let Some(Value::Array(types)) = o.get("type")
            && types.iter().any(|t| t == "null")
            && types.iter().any(|t| t == "array" || t == "object")
        {
            let kept: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
            let mut inner = std::mem::take(o);
            inner.insert(
                "type".into(),
                if kept.len() == 1 {
                    kept[0].clone()
                } else {
                    Value::Array(kept)
                },
            );
            let description = inner.remove("description");
            o.insert(
                "anyOf".into(),
                json!([Value::Object(inner), { "type": "null" }]),
            );
            if let Some(d) = description {
                o.insert("description".into(), d);
            }
            self.changes.push(format!(
                "{path}/type: nullable array or object written as anyOf with null"
            ));
        }
    }
}

/// Make a property schema accept `null`.
fn make_nullable(p: &mut Value) {
    let Some(o) = p.as_object_mut() else { return };
    match o.get_mut("type") {
        Some(Value::String(t)) => {
            let t = t.clone();
            o.insert("type".into(), json!([t, "null"]));
        }
        Some(Value::Array(a)) => {
            if !a.iter().any(|t| t == "null") {
                a.push(Value::String("null".into()));
            }
        }
        _ => {
            let existing = Value::Object(std::mem::take(o));
            o.insert("anyOf".into(), json!([existing, { "type": "null" }]));
        }
    }
}

/// When every alternative is an enum (or const) of one primitive type, the merged values.
fn enum_union(alts: &[Value]) -> Option<(String, Vec<Value>)> {
    let mut ty: Option<String> = None;
    let mut values: Vec<Value> = Vec::new();
    for a in alts {
        let o = a.as_object()?;
        let allowed: BTreeSet<&str> = ["type", "enum", "const", "description"]
            .into_iter()
            .collect();
        if !o.keys().all(|k| allowed.contains(k.as_str())) {
            return None;
        }
        let mut vals: Vec<Value> = match (o.get("enum"), o.get("const")) {
            (Some(Value::Array(e)), None) => e.clone(),
            (None, Some(c)) => vec![c.clone()],
            _ => return None,
        };
        let t = match o.get("type") {
            Some(Value::String(t)) => t.clone(),
            None => match vals.first()? {
                Value::String(_) => "string".into(),
                Value::Number(_) => "number".into(),
                Value::Bool(_) => "boolean".into(),
                _ => return None,
            },
            _ => return None,
        };
        if ty.as_ref().is_some_and(|x| *x != t) {
            return None;
        }
        ty = Some(t);
        for v in vals.drain(..) {
            if !values.contains(&v) {
                values.push(v);
            }
        }
    }
    Some((ty?, values))
}
