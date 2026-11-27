use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::sync::{Arc, LazyLock, Mutex};


mod html_escape {
    pub fn encode_safe(input: &str) -> String {
        input
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }
}

/// BladeContext — Menyimpan data dan variabel untuk dirender ke dalam template Blade.
/// Mendukung nested properties (dot-notation), array indexing, dan serialisasi data dinamis.
#[derive(Clone, Debug, Default)]
pub struct BladeContext {
    data: HashMap<String, Value>,
    stacks: Arc<Mutex<HashMap<String, Vec<String>>>>,
    once_set: Arc<Mutex<HashSet<String>>>,
}

impl BladeContext {
    pub fn new() -> Self {
        Self {
            data: HashMap::new(),
            stacks: Arc::new(Mutex::new(HashMap::new())),
            once_set: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Push konten ke stack tertentu (Laravel: @push('name'))
    pub fn push_to_stack(&self, name: &str, content: String) {
        if let Ok(mut lock) = self.stacks.lock() {
            lock.entry(name.to_string()).or_default().push(content);
        }
    }

    /// Prepend konten ke stack tertentu (Laravel: @prepend('name'))
    pub fn prepend_to_stack(&self, name: &str, content: String) {
        if let Ok(mut lock) = self.stacks.lock() {
            lock.entry(name.to_string()).or_default().insert(0, content);
        }
    }

    /// Ambil akumulasi konten stack tertentu (Laravel: @stack('name'))
    pub fn get_stack(&self, name: &str) -> String {
        if let Ok(lock) = self.stacks.lock() {
            if let Some(list) = lock.get(name) {
                return list.join("\n");
            }
        }
        String::new()
    }

    /// Cek dan tandai eksekusi blok @once
    pub fn check_and_mark_once(&self, id: &str) -> bool {
        if let Ok(mut lock) = self.once_set.lock() {
            lock.insert(id.to_string())
        } else {
            true
        }
    }

    /// Masukkan data serializable ke dalam context.
    pub fn insert<T: serde::Serialize>(&mut self, key: &str, value: T) {
        let clean_key = key.trim().trim_start_matches('$');
        if let Ok(val) = serde_json::to_value(value) {
            self.data.insert(clean_key.to_string(), val);
        }
    }

    /// Masukkan Value serde_json langsung.
    pub fn insert_val(&mut self, key: &str, value: Value) {
        let clean_key = key.trim().trim_start_matches('$');
        self.data.insert(clean_key.to_string(), value);
    }

    /// Ambil nilai variabel. Mendukung dot-notation (misal: "user.profile.name" atau "items.0.title").
    pub fn get(&self, key: &str) -> Option<&Value> {
        let key = key.trim().trim_start_matches('$');
        let normalized;
        let effective_key = if key.contains('[') || key.contains(".first()") || key.contains("->") {
            normalized = key
                .replace(".first()->", ".0.")
                .replace(".first().", ".0.")
                .replace("->first()->", ".0.")
                .replace("->first().", ".0.")
                .replace("->", ".")
                .replace("['", ".")
                .replace("[\"", ".")
                .replace("']", "")
                .replace("\"]", "")
                .replace('[', ".")
                .replace(']', "");
            &normalized
        } else {
            key
        };

        if !effective_key.contains('.') {
            return self.data.get(effective_key);
        }

        let parts: Vec<&str> = effective_key.split('.').filter(|p| !p.is_empty()).collect();
        if parts.is_empty() {
            return None;
        }
        let mut curr = self.data.get(parts[0])?;
        for part in &parts[1..] {
            if let Some(obj) = curr.as_object() {
                curr = obj.get(*part)?;
            } else if let Ok(idx) = part.parse::<usize>() {
                if let Some(arr) = curr.as_array() {
                    curr = arr.get(idx)?;
                } else {
                    return None;
                }
            } else {
                return None;
            }
        }
        Some(curr)
    }

    /// Cek apakah sebuah key ada di dalam context.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Cek apakah ada pesan error validasi untuk kolom tertentu.
    /// Cek apakah user memiliki izin tertentu (RBAC support di Blade view)
    pub fn check_user_can(&self, permission: &str) -> bool {
        if let Some(user_val) = self.get("user") {
            if let Some(role) = user_val.get("role").and_then(|r| r.as_str()) {
                if role == "superadmin" || role == "admin" {
                    return true;
                }
            }
            if let Some(perms) = user_val.get("permissions").and_then(|p| p.as_array()) {
                if perms.iter().any(|p| p.as_str() == Some(permission) || p.as_str() == Some("*")) {
                    return true;
                }
            }
        }
        if let Some(perms_val) = self.get("permissions") {
            if let Some(perms) = perms_val.as_array() {
                if perms.iter().any(|p| p.as_str() == Some(permission) || p.as_str() == Some("*")) {
                    return true;
                }
            }
        }
        false
    }

    /// Cek apakah user memiliki peran tertentu (RBAC support di Blade view)
    pub fn check_user_has_role(&self, role: &str) -> bool {
        if let Some(user_val) = self.get("user") {
            if let Some(r) = user_val.get("role").and_then(|r| r.as_str()) {
                return r == role || r == "superadmin";
            }
        }
        false
    }

    pub fn has_error(&self, field: &str) -> bool {
        self.get_error(field).is_some()
    }

    /// Ambil pesan error validasi untuk kolom tertentu jika ada.
    pub fn get_error(&self, field: &str) -> Option<String> {
        let clean_field = field.trim().trim_matches('\'').trim_matches('"');
        let errs = self.get("errors")?;
        if let Some(obj) = errs.as_object() {
            if let Some(val) = obj.get(clean_field) {
                match val {
                    Value::String(s) if !s.trim().is_empty() => return Some(s.clone()),
                    Value::Array(arr) if !arr.is_empty() => {
                        if let Some(first) = arr.first() {
                            if let Some(s) = first.as_str() {
                                return Some(s.to_string());
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// Cek apakah suatu nilai bertipe truthy menurut aturan Blade/Laravel.
    pub fn is_truthy(val: Option<&Value>) -> bool {
        match val {
            None => false,
            Some(Value::Null) => false,
            Some(Value::Bool(b)) => *b,
            Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
            Some(Value::String(s)) => {
                let trimmed = s.trim();
                !trimmed.is_empty() && trimmed != "0" && trimmed != "false"
            }
            Some(Value::Array(arr)) => !arr.is_empty(),
            Some(Value::Object(obj)) => !obj.is_empty(),
        }
    }

    /// Tokenizer untuk operator titik (konkatenasi string ala PHP/Laravel).
    /// Contoh: `'storage/images/meals/' . $meal.image` -> `["'storage/images/meals/'", "$meal.image"]`
    pub fn split_concat_tokens(expr: &str) -> Option<Vec<String>> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let chars: Vec<char> = expr.chars().collect();
        let len = chars.len();
        let mut has_concat = false;

        let mut i = 0;
        while i < len {
            let c = chars[i];
            if c == '\'' && !in_double {
                in_single = !in_single;
                current.push(c);
            } else if c == '"' && !in_single {
                in_double = !in_double;
                current.push(c);
            } else if c == '.' && !in_single && !in_double {
                let prev_is_space = i > 0 && chars[i - 1].is_whitespace();
                let next_is_space = i + 1 < len && chars[i + 1].is_whitespace();
                let prev_is_quote = current.trim().ends_with('\'') || current.trim().ends_with('"');
                let next_is_quote = i + 1 < len && (chars[i + 1] == '\'' || chars[i + 1] == '"');

                if prev_is_space || next_is_space || prev_is_quote || next_is_quote {
                    let trimmed = current.trim();
                    if !trimmed.is_empty() {
                        tokens.push(trimmed.to_string());
                    }
                    current.clear();
                    has_concat = true;
                } else {
                    current.push(c);
                }
            } else {
                current.push(c);
            }
            i += 1;
        }

        let trimmed = current.trim();
        if !trimmed.is_empty() {
            tokens.push(trimmed.to_string());
        }

        if has_concat && tokens.len() > 1 {
            Some(tokens)
        } else {
            None
        }
    }

    /// Helper mencari posisi pemisah koma terluar pada pemanggilan fungsi berargumen banyak.
    pub fn find_argument_split(s: &str) -> Option<usize> {
        let mut in_single = false;
        let mut in_double = false;
        let mut bracket_depth = 0;
        let mut paren_depth = 0;
        for (i, c) in s.char_indices() {
            match c {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                '[' if !in_single && !in_double => bracket_depth += 1,
                ']' if !in_single && !in_double && bracket_depth > 0 => bracket_depth -= 1,
                '(' if !in_single && !in_double => paren_depth += 1,
                ')' if !in_single && !in_double && paren_depth > 0 => paren_depth -= 1,
                ',' if !in_single && !in_double && bracket_depth == 0 && paren_depth == 0 => return Some(i),
                _ => {}
            }
        }
        None
    }

    /// Helper mencari token terluar (di luar tanda kurung/kutip).
    pub fn find_token_outside_parens(s: &str, token: &str) -> Option<usize> {
        let mut in_single = false;
        let mut in_double = false;
        let mut paren_depth = 0;
        let mut bracket_depth = 0;
        let tok_bytes = token.as_bytes();
        let tok_len = tok_bytes.len();
        let s_bytes = s.as_bytes();
        let len = s_bytes.len();

        let mut i = 0;
        while i < len {
            let b = s_bytes[i];
            match b {
                b'\'' if !in_double => in_single = !in_single,
                b'"' if !in_single => in_double = !in_double,
                b'(' if !in_single && !in_double => paren_depth += 1,
                b')' if !in_single && !in_double && paren_depth > 0 => paren_depth -= 1,
                b'[' if !in_single && !in_double => bracket_depth += 1,
                b']' if !in_single && !in_double && bracket_depth > 0 => bracket_depth -= 1,
                _ => {
                    if !in_single && !in_double && paren_depth == 0 && bracket_depth == 0 {
                        if i + tok_len <= len && &s_bytes[i..i + tok_len] == tok_bytes {
                            return Some(i);
                        }
                    }
                }
            }
            i += 1;
        }
        None
    }

    /// Helper memecah argumen berdasar koma terluar (misal: "a, [1, 2], 'x, y'") -> ["a", "[1, 2]", "'x, y'"]
    pub fn split_arguments(s: &str) -> Vec<String> {
        let mut args = Vec::new();
        let mut rem = s.trim();
        while !rem.is_empty() {
            if let Some(pos) = Self::find_argument_split(rem) {
                args.push(rem[..pos].trim().to_string());
                rem = rem[pos + 1..].trim();
            } else {
                args.push(rem.trim().to_string());
                break;
            }
        }
        args
    }

    /// Helper parsing array/hashmap sintaks Blade PHP:
    /// Contoh: "['title' => $title, 'status' => 'active', 'count' => 5]" atau "compact('item', 'title')"
    pub fn parse_blade_array(raw: &str, ctx: &BladeContext) -> HashMap<String, Value> {
        let mut res = HashMap::new();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return res;
        }

        // Dukungan compact('var1', 'var2')
        if trimmed.starts_with("compact(") && trimmed.ends_with(')') {
            let inside = &trimmed[8..trimmed.len() - 1];
            for var in Self::split_arguments(inside) {
                let var_name = var.trim().trim_matches('\'').trim_matches('"').trim_start_matches('$');
                if let Some(v) = ctx.get(var_name) {
                    res.insert(var_name.to_string(), v.clone());
                }
            }
            return res;
        }

        let inner = if (trimmed.starts_with('[') && trimmed.ends_with(']')) || (trimmed.starts_with("array(") && trimmed.ends_with(')')) {
            if trimmed.starts_with('[') {
                &trimmed[1..trimmed.len() - 1]
            } else {
                &trimmed[6..trimmed.len() - 1]
            }
        } else {
            trimmed
        };

        let pairs = Self::split_arguments(inner);
        for pair in pairs {
            let pair_trim = pair.trim();
            if pair_trim.is_empty() {
                continue;
            }
            if let Some(arrow_pos) = Self::find_token_outside_parens(pair_trim, "=>") {
                let key_raw = pair_trim[..arrow_pos].trim().trim_matches('\'').trim_matches('"');
                let val_raw = pair_trim[arrow_pos + 2..].trim();
                let val_clean = val_raw.trim_start_matches('$');
                if let Some(v) = ctx.get(val_clean) {
                    res.insert(key_raw.to_string(), v.clone());
                } else {
                    let evaled = ctx.eval_val(val_raw);
                    if let Ok(num) = evaled.parse::<i64>() {
                        res.insert(key_raw.to_string(), serde_json::json!(num));
                    } else if let Ok(f) = evaled.parse::<f64>() {
                        res.insert(key_raw.to_string(), serde_json::json!(f));
                    } else if evaled == "true" {
                        res.insert(key_raw.to_string(), serde_json::json!(true));
                    } else if evaled == "false" {
                        res.insert(key_raw.to_string(), serde_json::json!(false));
                    } else {
                        res.insert(key_raw.to_string(), serde_json::json!(evaled));
                    }
                }
            }
        }
        res
    }

    /// Helper format angka ribuan (e.g. 15000 -> "15.000" atau "15,000").
    pub fn format_thousands(val_str: &str, sep: char) -> String {
        let trimmed = val_str.trim();
        if let Ok(num) = trimmed.parse::<i64>() {
            let num_str = num.abs().to_string();
            let mut res = String::new();
            let len = num_str.len();
            for (i, c) in num_str.chars().enumerate() {
                if i > 0 && (len - i) % 3 == 0 {
                    res.push(sep);
                }
                res.push(c);
            }
            if num < 0 {
                format!("-{}", res)
            } else {
                res
            }
        } else if let Ok(num) = trimmed.parse::<f64>() {
            let rounded = num.round() as i64;
            Self::format_thousands(&rounded.to_string(), sep)
        } else {
            trimmed.to_string()
        }
    }

    /// Helper parsing komponen tanggal (day, month, year) dari string format database.
    pub fn parse_date_parts(raw: &str) -> Option<(u32, u32, i32)> {
        let trimmed = raw.trim();
        let date_part = trimmed.split_whitespace().next().unwrap_or(trimmed);
        let sep = if date_part.contains('-') { '-' } else if date_part.contains('/') { '/' } else { return None; };
        let parts: Vec<&str> = date_part.split(sep).collect();
        if parts.len() == 3 {
            if parts[0].len() == 4 {
                let year = parts[0].parse::<i32>().ok()?;
                let month = parts[1].parse::<u32>().ok()?;
                let day = parts[2].parse::<u32>().ok()?;
                if (1..=12).contains(&month) && (1..=31).contains(&day) {
                    return Some((day, month, year));
                }
            } else if parts[2].len() == 4 {
                let day = parts[0].parse::<u32>().ok()?;
                let month = parts[1].parse::<u32>().ok()?;
                let year = parts[2].parse::<i32>().ok()?;
                if (1..=12).contains(&month) && (1..=31).contains(&day) {
                    return Some((day, month, year));
                }
            }
        }
        None
    }

    /// Helper format tanggal PHP (e.g. 'd F Y', 'd M', 'd M Y').
    pub fn format_php_date(format_str: &str, raw_date: &str) -> String {
        let (day, month, year) = if let Some(parts) = Self::parse_date_parts(raw_date) {
            parts
        } else {
            let now = chrono::Local::now();
            use chrono::Datelike;
            (now.day(), now.month(), now.year())
        };

        const ENG_MONTHS_SHORT: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        const ENG_MONTHS_FULL: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

        let m_idx = (month.max(1).min(12) - 1) as usize;

        let mut res = String::new();
        for c in format_str.chars() {
            match c {
                'd' => res.push_str(&format!("{:02}", day)),
                'j' => res.push_str(&day.to_string()),
                'F' => res.push_str(ENG_MONTHS_FULL[m_idx]),
                'M' => res.push_str(ENG_MONTHS_SHORT[m_idx]),
                'm' => res.push_str(&format!("{:02}", month)),
                'Y' => res.push_str(&year.to_string()),
                'y' => res.push_str(&format!("{:02}", year.rem_euclid(100))),
                other => res.push(other),
            }
        }
        res
    }

    /// Helper format tanggal Indonesia (e.g. 20 Mei 2024).
    pub fn format_tanggal_indo(raw_date: &str) -> String {
        if let Some((day, month, year)) = Self::parse_date_parts(raw_date) {
            const INDO_MONTHS_FULL: [&str; 12] = ["Januari", "Februari", "Maret", "April", "Mei", "Juni", "Juli", "Agustus", "September", "Oktober", "November", "Desember"];
            let m_idx = (month.max(1).min(12) - 1) as usize;
            format!("{} {} {}", day, INDO_MONTHS_FULL[m_idx], year)
        } else {
            raw_date.trim().to_string()
        }
    }

    /// Helper format tanggal Inggris (e.g. 20 May 2024).
    pub fn format_tanggal_eng(raw_date: &str) -> String {
        if let Some((day, month, year)) = Self::parse_date_parts(raw_date) {
            const ENG_MONTHS_FULL: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
            let m_idx = (month.max(1).min(12) - 1) as usize;
            format!("{} {} {}", day, ENG_MONTHS_FULL[m_idx], year)
        } else {
            raw_date.trim().to_string()
        }
    }

    /// Evaluasi nilai ekspresi variabel/literal menjadi String.
    /// Mendukung null coalescing: `$name ?? 'Default'`
    pub fn eval_val(&self, expr: &str) -> String {
        let expr = expr.trim();
        if expr.is_empty() {
            return String::new();
        }

        // Hapus tanda kurung terluar jika benar-benar melingkupi seluruh ekspresi: ( expr )
        if expr.starts_with('(') && expr.ends_with(')') {
            let mut depth = 0;
            let mut can_strip = true;
            for (i, c) in expr.char_indices() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 && i < expr.len() - 1 {
                        can_strip = false;
                        break;
                    }
                }
            }
            if can_strip && depth == 0 {
                let inner = expr[1..expr.len() - 1].trim();
                return self.eval_val(inner);
            }
        }

        // Operasi biner aritmatika sederhana di luar tanda kurung: expr1 - expr2 atau expr1 + expr2
        if let Some(idx) = Self::find_token_outside_parens(expr, "-") {
            let left_val = self.eval_val(&expr[..idx]);
            let right_val = self.eval_val(&expr[idx + 1..]);
            if let (Ok(n1), Ok(n2)) = (left_val.parse::<f64>(), right_val.parse::<f64>()) {
                return (n1 - n2).to_string();
            }
        }
        if let Some(idx) = Self::find_token_outside_parens(expr, "+") {
            let left_val = self.eval_val(&expr[..idx]);
            let right_val = self.eval_val(&expr[idx + 1..]);
            if let (Ok(n1), Ok(n2)) = (left_val.parse::<f64>(), right_val.parse::<f64>()) {
                return (n1 + n2).to_string();
            }
        }

        // Null coalescing di luar tanda kurung: expr1 ?? expr2
        if let Some(idx) = Self::find_token_outside_parens(expr, "??") {
            let left = expr[..idx].trim();
            let right = expr[idx + 2..].trim();
            let val = self.eval_val(left);
            if !val.is_empty() {
                return val;
            } else {
                return self.eval_val(right);
            }
        }

        // Operator ternary: cond ? expr1 : expr2
        if let Some(q_idx) = Self::find_token_outside_parens(expr, "?") {
            if !expr[q_idx..].starts_with("??") && (q_idx == 0 || &expr[q_idx - 1..q_idx] != "?") {
                if let Some(colon_rel) = Self::find_token_outside_parens(&expr[q_idx + 1..], ":") {
                    let colon_idx = q_idx + 1 + colon_rel;
                    let cond = expr[..q_idx].trim();
                    let true_val = expr[q_idx + 1..colon_idx].trim();
                    let false_val = expr[colon_idx + 1..].trim();
                    if self.eval_truthy(cond) {
                        return self.eval_val(true_val);
                    } else {
                        return self.eval_val(false_val);
                    }
                }
            }
        }

        // String literal: '...' atau "..."
        if (expr.starts_with('\'') && expr.ends_with('\'') && expr.len() >= 2)
            || (expr.starts_with('"') && expr.ends_with('"') && expr.len() >= 2)
        {
            return expr[1..expr.len() - 1].to_string();
        }

        // Boolean literal
        if expr == "true" {
            return "true".to_string();
        }
        if expr == "false" {
            return "false".to_string();
        }

        // Number literal
        if expr.parse::<f64>().is_ok() {
            return expr.to_string();
        }

        // Helper PHP/Blade: count($var)
        if expr.starts_with("count(") && expr.ends_with(')') {
            let inner = expr[6..expr.len() - 1].trim();
            let var = inner.trim_start_matches('$');
            if let Some(val) = self.get(var) {
                return match val {
                    Value::Array(a) => a.len().to_string(),
                    Value::Object(o) => o.len().to_string(),
                    Value::String(s) => if s.is_empty() { "0".to_string() } else { s.len().to_string() },
                    Value::Null => "0".to_string(),
                    _ => "1".to_string(),
                };
            }
            return "0".to_string();
        }

        // Method call: $var.count() atau $var->count()
        if let Some(prefix) = expr.strip_suffix(".count()").or_else(|| expr.strip_suffix("->count()")) {
            let var = prefix.trim().trim_start_matches('$');
            if let Some(val) = self.get(var) {
                return match val {
                    Value::Array(a) => a.len().to_string(),
                    Value::Object(o) => o.len().to_string(),
                    Value::String(s) => if s.is_empty() { "0".to_string() } else { s.len().to_string() },
                    Value::Null => "0".to_string(),
                    _ => "1".to_string(),
                };
            }
            return "0".to_string();
        }

        // Helper Laravel: old('field', 'default')
        if expr.starts_with("old(") && expr.ends_with(')') {
            let inner = expr[4..expr.len() - 1].trim();
            let parts: Vec<&str> = inner.splitn(2, ',').collect();
            let field = parts[0].trim().trim_matches('\'').trim_matches('"');
            let default_val = if parts.len() > 1 {
                self.eval_val(parts[1].trim())
            } else {
                String::new()
            };

            if let Some(old_val) = self.get("old") {
                if let Some(obj) = old_val.as_object() {
                    if let Some(val) = obj.get(field) {
                        match val {
                            Value::String(s) => return s.clone(),
                            Value::Number(n) => return n.to_string(),
                            Value::Bool(b) => return b.to_string(),
                            Value::Null => {},
                            other => return other.to_string(),
                        }
                    }
                }
            }
            return default_val;
        }

        // Helper Laravel: asset('path')
        if expr.starts_with("asset(") && expr.ends_with(')') {
            let inner = expr[6..expr.len() - 1].trim();
            let raw_eval = self.eval_val(inner);
            let raw = raw_eval.trim().trim_matches('\'').trim_matches('"');
            if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("//") {
                return raw.to_string();
            }
            let clean = raw.trim_start_matches('/');
            return format!("/{}", clean);
        }

        // Helper Laravel: mix('path')
        if expr.starts_with("mix(") && expr.ends_with(')') {
            let inner = expr[4..expr.len() - 1].trim();
            let raw_eval = self.eval_val(inner);
            let raw = raw_eval.trim().trim_matches('\'').trim_matches('"');
            let clean = raw.trim_start_matches('/');
            return format!("/{}", clean);
        }

        // Helper Laravel: route('name', ...)
        if expr.starts_with("route(") && expr.ends_with(')') {
            let inner = expr[6..expr.len() - 1].trim();
            let (first_arg, second_arg) = if let Some(comma_pos) = Self::find_argument_split(inner) {
                (&inner[..comma_pos], Some(&inner[comma_pos + 1..]))
            } else {
                (inner, None)
            };
            let route_name = first_arg.trim().trim_matches('\'').trim_matches('"');
            if let Some(second) = second_arg {
                let second = second.trim();
                let param_val = if second.starts_with('[') && second.ends_with(']') {
                    let inside = &second[1..second.len() - 1];
                    if let Some(arrow_pos) = inside.find("=>") {
                        let rhs = inside[arrow_pos + 2..].trim();
                        self.eval_val(rhs)
                    } else {
                        self.eval_val(inside.trim())
                    }
                } else {
                    self.eval_val(second)
                };
                let clean_name = route_name.trim_start_matches('/');
                let clean_param = param_val.trim().trim_matches('\'').trim_matches('"');
                if !clean_param.is_empty() {
                    return format!("/{}/{}", clean_name, clean_param);
                } else {
                    return format!("/{}", clean_name);
                }
            } else {
                let clean_name = route_name.trim_start_matches('/');
                return format!("/{}", clean_name);
            }
        }

        // Helper Laravel: url('path')
        if expr.starts_with("url(") && expr.ends_with(')') {
            let inner = expr[4..expr.len() - 1].trim();
            let raw_eval = self.eval_val(inner);
            let raw = raw_eval.trim().trim_matches('\'').trim_matches('"');
            if raw.starts_with("http://") || raw.starts_with("https://") {
                return raw.to_string();
            }
            let clean = raw.trim_start_matches('/');
            return format!("/{}", clean);
        }

        // Helper Laravel: request()->is(...) / request()->routeIs(...)
        if (expr.starts_with("request()->is(") || expr.starts_with("request()->routeIs(") || expr.starts_with("request.is(") || expr.starts_with("request.routeIs(")) && expr.ends_with(')') {
            let open = expr.rfind('(').unwrap();
            let inner = expr[open + 1..expr.len() - 1].trim();
            let pattern = self.eval_val(inner).trim().trim_matches('\'').trim_matches('"').to_string();
            let curr_path = self.get("request_path")
                .or_else(|| self.get("current_route"))
                .or_else(|| self.get("path"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let clean_pattern = pattern.trim_start_matches('/');
            let clean_curr = curr_path.trim_start_matches('/');
            let matches = if clean_pattern.ends_with('*') {
                clean_curr.starts_with(clean_pattern.trim_end_matches('*'))
            } else {
                clean_curr == clean_pattern
            };
            return if matches { "1".to_string() } else { String::new() };
        }

        // Helper Laravel: session('key', 'default')
        if expr.starts_with("session(") && expr.ends_with(')') {
            let inner = expr[8..expr.len() - 1].trim();
            if inner.is_empty() {
                return String::new();
            }
            let (key_arg, def_arg) = if let Some(comma) = Self::find_argument_split(inner) {
                (inner[..comma].trim(), Some(inner[comma + 1..].trim()))
            } else {
                (inner, None)
            };
            let key = key_arg.trim_matches('\'').trim_matches('"');
            let session_key = format!("session.{}", key);
            if let Some(val) = self.get(&session_key).or_else(|| self.get(key)) {
                match val {
                    Value::String(s) => return s.clone(),
                    Value::Number(n) => return n.to_string(),
                    Value::Bool(b) => return b.to_string(),
                    Value::Null => {},
                    other => return other.to_string(),
                }
            }
            if let Some(def) = def_arg {
                return self.eval_val(def);
            }
            return String::new();
        }

        // Helper Laravel: auth()->check(), auth()->id(), auth()->user()->prop, Auth::user()->prop
        if expr == "auth()->check()" {
            return if self.get("user").is_some() { "1".to_string() } else { String::new() };
        }
        if expr == "auth()->id()" {
            if let Some(id) = self.get("user.id") {
                return id.to_string();
            }
            return String::new();
        }
        if expr.starts_with("auth()->user()->") {
            let prop = &expr[16..];
            let key = format!("user.{}", prop);
            if let Some(val) = self.get(&key) {
                match val {
                    Value::String(s) => return s.clone(),
                    Value::Number(n) => return n.to_string(),
                    Value::Bool(b) => return b.to_string(),
                    _ => return val.to_string(),
                }
            }
            return String::new();
        }
        if expr.starts_with("Auth::user()->") {
            let prop = &expr[14..];
            let key = format!("user.{}", prop);
            if let Some(val) = self.get(&key) {
                match val {
                    Value::String(s) => return s.clone(),
                    Value::Number(n) => return n.to_string(),
                    Value::Bool(b) => return b.to_string(),
                    _ => return val.to_string(),
                }
            }
            return String::new();
        }

        // Helper Laravel: Str::limit($text, limit), str_limit($text, limit), Str::slug($text)
        if (expr.starts_with("Str::limit(") || expr.starts_with("str_limit(")) && expr.ends_with(')') {
            let open = expr.find('(').unwrap();
            let inner = expr[open + 1..expr.len() - 1].trim();
            let parts = Self::split_arguments(inner);
            if !parts.is_empty() {
                let text = self.eval_val(&parts[0]);
                let limit: usize = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(100);
                let end_str = parts.get(2).map(|p| p.trim().trim_matches('\'').trim_matches('"')).unwrap_or("...");
                let chars: Vec<char> = text.chars().collect();
                if chars.len() > limit {
                    let truncated: String = chars[..limit].iter().collect();
                    return format!("{}{}", truncated, end_str);
                } else {
                    return text;
                }
            }
            return String::new();
        }
        if expr.starts_with("Str::slug(") && expr.ends_with(')') {
            let inner = expr[10..expr.len() - 1].trim();
            let text = self.eval_val(inner);
            return BladeEngine::slug(&text);
        }

        // String Helpers: strtoupper, strtolower, ucfirst, ucwords, substr, str_replace
        if expr.starts_with("strtoupper(") && expr.ends_with(')') {
            let inner = expr[11..expr.len() - 1].trim();
            return self.eval_val(inner).to_uppercase();
        }
        if expr.starts_with("strtolower(") && expr.ends_with(')') {
            let inner = expr[11..expr.len() - 1].trim();
            return self.eval_val(inner).to_lowercase();
        }
        if expr.starts_with("ucfirst(") && expr.ends_with(')') {
            let inner = expr[8..expr.len() - 1].trim();
            let s = self.eval_val(inner);
            let mut chars = s.chars();
            return match chars.next() {
                None => String::new(),
                Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
            };
        }
        if expr.starts_with("ucwords(") && expr.ends_with(')') {
            let inner = expr[8..expr.len() - 1].trim();
            let s = self.eval_val(inner);
            return s.split_whitespace()
                .map(|w| {
                    let mut c = w.chars();
                    match c.next() {
                        None => String::new(),
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
        }
        if expr.starts_with("substr(") && expr.ends_with(')') {
            let inner = expr[7..expr.len() - 1].trim();
            let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
            if !parts.is_empty() {
                let s = self.eval_val(parts[0]);
                let start: usize = parts.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
                let chars: Vec<char> = s.chars().collect();
                if start >= chars.len() {
                    return String::new();
                }
                if let Some(len_str) = parts.get(2) {
                    if let Ok(len) = len_str.parse::<usize>() {
                        let end = (start + len).min(chars.len());
                        return chars[start..end].iter().collect();
                    }
                }
                return chars[start..].iter().collect();
            }
            return String::new();
        }
        if expr.starts_with("str_replace(") && expr.ends_with(')') {
            let inner = expr[12..expr.len() - 1].trim();
            let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
            if parts.len() >= 3 {
                let search = self.eval_val(parts[0]);
                let replace = self.eval_val(parts[1]);
                let subject = self.eval_val(parts[2]);
                return subject.replace(&search, &replace);
            }
            return String::new();
        }

        // Date Helpers: date('format', strtotime($var)), tanggalOnly($var), tanggalInggris($var)
        if expr.starts_with("date(") && expr.ends_with(')') {
            let inner = expr[5..expr.len() - 1].trim();
            let (first_arg, second_arg) = if let Some(comma_pos) = Self::find_argument_split(inner) {
                (&inner[..comma_pos], Some(&inner[comma_pos + 1..]))
            } else {
                (inner, None)
            };
            let format_str = first_arg.trim().trim_matches('\'').trim_matches('"');
            let raw_date = if let Some(second) = second_arg {
                let second = second.trim();
                if second.starts_with("strtotime(") && second.ends_with(')') {
                    let inside = &second[10..second.len() - 1];
                    self.eval_val(inside.trim())
                } else {
                    self.eval_val(second)
                }
            } else {
                String::new()
            };
            return Self::format_php_date(format_str, &raw_date);
        }
        if expr.starts_with("tanggalOnly(") && expr.ends_with(')') {
            let inner = expr[12..expr.len() - 1].trim();
            let raw_date = self.eval_val(inner);
            return Self::format_tanggal_indo(&raw_date);
        }
        if expr.starts_with("tanggalInggris(") && expr.ends_with(')') {
            let inner = expr[15..expr.len() - 1].trim();
            let raw_date = self.eval_val(inner);
            return Self::format_tanggal_eng(&raw_date);
        }

        // Currency / Number Helpers: uang, ribuan, number_format
        if (expr.starts_with("uang(") || expr.starts_with("ribuan(")) && expr.ends_with(')') {
            let open = expr.find('(').unwrap();
            let inner = expr[open + 1..expr.len() - 1].trim();
            let val_str = self.eval_val(inner);
            return Self::format_thousands(&val_str, '.');
        }
        if expr.starts_with("number_format(") && expr.ends_with(')') {
            let inner = expr[14..expr.len() - 1].trim();
            let parts: Vec<&str> = inner.split(',').map(|p| p.trim()).collect();
            let val_str = self.eval_val(parts[0]);
            let sep = if parts.len() >= 4 {
                parts[3].trim_matches('\'').trim_matches('"').chars().next().unwrap_or(',')
            } else {
                ','
            };
            return Self::format_thousands(&val_str, sep);
        }

        // Localization & Translation Helpers: __, trans, app()->getLocale()
        if (expr.starts_with("__(") || expr.starts_with("trans(")) && expr.ends_with(')') {
            let open = expr.find('(').unwrap();
            let inner = expr[open + 1..expr.len() - 1].trim();
            let parts: Vec<&str> = inner.splitn(2, ',').collect();
            let key = parts[0].trim().trim_matches('\'').trim_matches('"');
            return key.to_string();
        }
        if expr == "app()->getLocale()" || expr == "app()->currentLocale()" {
            return "id".to_string();
        }
        if expr == "csrf_token()" || expr == "csrf_token" {
            if let Some(t) = self.get("csrf_token") {
                if let Some(s) = t.as_str() {
                    return s.to_string();
                }
            }
            return String::new();
        }

        // Konkatenasi string gaya Laravel/PHP dengan operator titik (.)
        // Contoh: 'storage/images/meals/' . $meal.image
        if let Some(tokens) = Self::split_concat_tokens(expr) {
            let mut res = String::new();
            for tok in tokens {
                res.push_str(&self.eval_val(&tok));
            }
            return res;
        }

        // Variable lookup
        if let Some(val) = self.get(expr) {
            match val {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                Value::Bool(b) => b.to_string(),
                Value::Null => String::new(),
                v => v.to_string(),
            }
        } else {
            String::new()
        }
    }

    /// Evaluasi kondisi boolean (truthy) untuk direktif @if, @unless, @checked, dll.
    pub fn eval_truthy(&self, expr: &str) -> bool {
        let expr = expr.trim();
        if expr.is_empty() {
            return false;
        }

        // Hapus tanda kurung terluar jika benar-benar melingkupi seluruh ekspresi: ( expr )
        if expr.starts_with('(') && expr.ends_with(')') {
            let mut depth = 0;
            let mut can_strip = true;
            for (i, c) in expr.char_indices() {
                if c == '(' {
                    depth += 1;
                } else if c == ')' {
                    depth -= 1;
                    if depth == 0 && i < expr.len() - 1 {
                        can_strip = false;
                        break;
                    }
                }
            }
            if can_strip && depth == 0 {
                let inner = &expr[1..expr.len() - 1];
                return self.eval_truthy(inner);
            }
        }

        // Logical OR: ||
        if let Some(idx) = expr.find("||") {
            let left = &expr[..idx];
            let right = &expr[idx + 2..];
            return self.eval_truthy(left) || self.eval_truthy(right);
        }

        // Logical AND: &&
        if let Some(idx) = expr.find("&&") {
            let left = &expr[..idx];
            let right = &expr[idx + 2..];
            return self.eval_truthy(left) && self.eval_truthy(right);
        }

        // Unary NOT: !expr
        if expr.starts_with('!') {
            return !self.eval_truthy(&expr[1..]);
        }

        // Special helpers:
        // auth() / auth
        if expr == "auth" || expr == "auth()" {
            let uid = self.get("user_id").or_else(|| self.get("user")).or_else(|| self.get("auth"));
            return Self::is_truthy(uid);
        }
        // guest() / guest
        if expr == "guest" || expr == "guest()" {
            let uid = self.get("user_id").or_else(|| self.get("user")).or_else(|| self.get("auth"));
            return !Self::is_truthy(uid);
        }
        // can('permission')
        if expr.starts_with("can(") && expr.ends_with(')') {
            let perm = expr[4..expr.len() - 1].trim().trim_matches('\'').trim_matches('"');
            return self.check_user_can(perm);
        }
        // cannot('permission')
        if expr.starts_with("cannot(") && expr.ends_with(')') {
            let perm = expr[7..expr.len() - 1].trim().trim_matches('\'').trim_matches('"');
            return !self.check_user_can(perm);
        }
        // role('role')
        if expr.starts_with("role(") && expr.ends_with(')') {
            let role_name = expr[5..expr.len() - 1].trim().trim_matches('\'').trim_matches('"');
            return self.check_user_has_role(role_name);
        }
        // isset($var)
        if expr.starts_with("isset(") && expr.ends_with(')') {
            let var = &expr[6..expr.len() - 1].trim();
            return self.get(var).is_some() && !self.get(var).unwrap().is_null();
        }
        // empty($var)
        if expr.starts_with("empty(") && expr.ends_with(')') {
            let var = &expr[6..expr.len() - 1].trim();
            return !Self::is_truthy(self.get(var));
        }
        // count($var)
        if expr.starts_with("count(") && expr.ends_with(')') {
            return self.eval_val(expr).parse::<f64>().map(|n| n > 0.0).unwrap_or(false);
        }
        // $var.count() / $var->count()
        if expr.ends_with(".count()") || expr.ends_with("->count()") {
            return self.eval_val(expr).parse::<f64>().map(|n| n > 0.0).unwrap_or(false);
        }
        // production / production()
        if expr == "production" || expr == "production()" {
            let env = self.get("env").and_then(|v| v.as_str()).unwrap_or("");
            return env == "production" || env == "prod" || std::env::var("APP_ENV").unwrap_or_default() == "production";
        }
        // env('local', 'staging')
        if expr.starts_with("env(") && expr.ends_with(')') {
            let args = &expr[4..expr.len() - 1];
            let current_env = self.get("env").and_then(|v| v.as_str())
                .unwrap_or_else(|| "local");
            for arg in args.split(',') {
                let clean_arg = arg.trim().trim_matches('\'').trim_matches('"');
                if clean_arg == current_env {
                    return true;
                }
            }
            return false;
        }

        // error('field') / hasError('field')
        if (expr.starts_with("error(") && expr.ends_with(')')) || (expr.starts_with("hasError(") && expr.ends_with(')')) {
            let open = expr.find('(').unwrap();
            let field = expr[open + 1..expr.len() - 1].trim();
            return self.has_error(field);
        }

        // anyErrors / errors.any / $errors->any() / errors
        if expr == "errors" || expr == "errors()" || expr == "$errors->any()" || expr == "errors.any" || expr == "anyErrors" {
            if let Some(val) = self.get("errors") {
                if let Some(obj) = val.as_object() {
                    return !obj.is_empty();
                }
            }
            return false;
        }

        // Perbandingan: ===, ==, !=, >=, <=, >, < (di luar tanda kurung/kutip)
        for op in &["===", "==", "!=", ">=", "<=", ">", "<"] {
            if let Some(idx) = Self::find_token_outside_parens(expr, op) {
                // Lewati '>' jika merupakan bagian dari operator panah '->'
                if *op == ">" && idx > 0 && expr.as_bytes()[idx - 1] == b'-' {
                    continue;
                }
                let left = self.eval_val(&expr[..idx]);
                let right = self.eval_val(&expr[idx + op.len()..]);

                // Perbandingan numerik jika kedua sisi angka
                if let (Ok(n1), Ok(n2)) = (left.parse::<f64>(), right.parse::<f64>()) {
                    return match *op {
                        "===" | "==" => (n1 - n2).abs() < f64::EPSILON,
                        "!=" => (n1 - n2).abs() >= f64::EPSILON,
                        ">=" => n1 >= n2,
                        "<=" => n1 <= n2,
                        ">" => n1 > n2,
                        "<" => n1 < n2,
                        _ => false,
                    };
                }

                // Perbandingan string
                return match *op {
                    "===" | "==" => left == right,
                    "!=" => left != right,
                    ">=" => left >= right,
                    "<=" => left <= right,
                    ">" => left > right,
                    "<" => left < right,
                    _ => false,
                };
            }
        }

        // Boolean literal
        if expr == "true" {
            return true;
        }
        if expr == "false" {
            return false;
        }

        if let Some(val) = self.get(expr) {
            return Self::is_truthy(Some(val));
        }

        let evaled = self.eval_val(expr);
        if !evaled.is_empty() && evaled != "0" && evaled != "false" {
            return true;
        }

        false
    }
}

/// Interface runner untuk mengeksekusi plugin pihak ketiga (misal WASM Sandbox / Indonesian Script)
pub trait BladePluginRunner: Send + Sync {
    fn call_filter(&self, plugin_name: &str, func_name: &str, arg: &str) -> Result<String, String> {
        let _ = (plugin_name, func_name, arg);
        Err("No plugin runner configured".to_string())
    }

    fn eval_is_script(&self, script_path: &str, func_name: &str, args: &[Value]) -> Result<Value, String> {
        let _ = (script_path, func_name, args);
        Err("No script runner configured".to_string())
    }
}

/// Runner default kosong (No-op)
#[derive(Default, Clone)]
pub struct NoopPluginRunner;
impl BladePluginRunner for NoopPluginRunner {}

/// BladeEngine — Pure Native Laravel Blade Templating Engine for Rust.
/// Sangat cepat, tanpa alokasi berat, mendukung layout inheritance, loop variables,
/// conditional directives, integrasi Indonesian Script, serta Filter & Ekstensi WASM Sandbox.
pub struct BladeEngine {
    pub templates: HashMap<String, String>,
    pub plugin_runner: Arc<dyn BladePluginRunner>,
    pub wasm_filters: HashMap<String, (String, String)>,
}

impl Default for BladeEngine {
    fn default() -> Self {
        Self::empty()
    }
}



pub type CustomDirectiveHandler = std::sync::Arc<dyn Fn(&str, &BladeEngine, &BladeContext) -> String + Send + Sync>;

#[derive(Clone)]
pub struct RegisteredDirective {
    pub name: String,
    pub re_args: Regex,
    pub re_no_args: Regex,
    pub handler: CustomDirectiveHandler,
}

static RE_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\{\{--.*?--\}\}").unwrap());
static RE_METHOD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@method\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_WASM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@wasm\s*\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]\s*(?:,\s*(.*?))?\s*\)"#).unwrap());
static RE_EVAL_IS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@eval_is\s*\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]\s*(?:,\s*(.*?))?\s*\)"#).unwrap());
static RE_RAW: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{!!\s*(.*?)\s*!!\}").unwrap());
static RE_VAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{\{\s*(.*?)\s*\}\}").unwrap());
static RE_UNLESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@unless\s*\((.*?)\)").unwrap());
static RE_ISSET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@isset\s*\((.*?)\)").unwrap());
static RE_EMPTY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@empty\s*\((.*?)\)").unwrap());
static RE_AUTH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@auth\b").unwrap());
static RE_GUEST: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@guest\b").unwrap());
static RE_CAN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@can\s*\((.*?)\)").unwrap());
static RE_CANNOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@cannot\s*\((.*?)\)").unwrap());
static RE_ROLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@role\s*\((.*?)\)").unwrap());
static RE_PROD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@production\b").unwrap());
static RE_ENV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@env\s*\((.*?)\)").unwrap());
static RE_EXTENDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@extends\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_SHORT_SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@section\s*\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_SECTION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@section\s*\(\s*['"]([^'"]+)['"]\s*\)(.*?)@(?:endsection|stop|show)"#).unwrap());
static RE_YIELD_DEFAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@yield\s*\(\s*['"]([^'"]+)['"]\s*,\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_YIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@yield\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_HAS_SEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@hasSection\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_PUSH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@push\s*\(\s*['"]([^'"]+)['"]\s*\)(.*?)@endpush"#).unwrap());
static RE_PREPEND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@prepend\s*\(\s*['"]([^'"]+)['"]\s*\)(.*?)@endprepend"#).unwrap());
static RE_STACK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@stack\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());
static RE_ONCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@once(.*?)@endonce"#).unwrap());
static RE_INC_WHEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@includeWhen\s*\((.*?)\)"#).unwrap());
static RE_INC_UNLESS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@includeUnless\s*\((.*?)\)"#).unwrap());
static RE_INC_IF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@includeIf\s*\((.*?)\)"#).unwrap());
static RE_INC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@include\s*\((.*?)\)"#).unwrap());
static RE_VITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"@vite\s*\((.*?)\)"#).unwrap());
static RE_COMPONENT_SELF: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<x-([a-zA-Z0-9_\.-]+)([^>]*?)/>"#).unwrap());
static RE_COMPONENT_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<x-([a-zA-Z0-9_\.-]+)([^>]*?)>"#).unwrap());
static RE_COMPONENT_SLOT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)<x-slot(?::([a-zA-Z0-9_\.-]+)|(?:\s+name=['"]([a-zA-Z0-9_\.-]+)['"]))>(.*?)</x-slot>"#).unwrap());
static RE_COMPONENT_ATTR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?::([a-zA-Z0-9_\.-]+)|([a-zA-Z0-9_\.-]+))\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
static RE_FOREACH_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@foreach\s*\((.*?)\)").unwrap());
static RE_FORELSE_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@forelse\s*\((.*?)\)").unwrap());
static RE_FOR_OPEN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@for\s*\((.*?)\)").unwrap());
static RE_INIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$?(\w+)\s*=\s*(\d+)").unwrap());
static RE_COND: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<\s*(\d+)").unwrap());
static RE_SWITCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@switch\s*\((.*?)\)(.*?)@endswitch"#).unwrap());
static RE_CASES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@case\s*\(\s*['"]?([^'"]+)['"]?\s*\)(.*?)(?:@break|@case|@default|$)"#).unwrap());
static RE_DEFAULT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@default(.*?)(?:@break|$)"#).unwrap());
static RE_ERROR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?s)@error\s*\(\s*['"]([^'"]+)['"]\s*\)(.*?)@enderror"#).unwrap());
static RE_CHECKED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@checked\s*\((.*?)\)").unwrap());
static RE_SELECTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@selected\s*\((.*?)\)").unwrap());
static RE_DISABLED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@disabled\s*\((.*?)\)").unwrap());
static RE_READONLY: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@readonly\s*\((.*?)\)").unwrap());
static RE_REQUIRED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@required\s*\((.*?)\)").unwrap());
static RE_CLASS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@class\s*\(\s*\[(.*?)\]\s*\)").unwrap());
static RE_STYLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@style\s*\(\s*\[(.*?)\]\s*\)").unwrap());
static RE_JSON: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@json\s*\((.*?)\)").unwrap());
static RE_DUMP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"@(?:dump|dd)\s*\((.*?)\)").unwrap());
static RE_IS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?:@is|@script)\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap());

static GLOBAL_DIRECTIVES: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, RegisteredDirective>>> = std::sync::OnceLock::new();

/// Daftarkan custom directive ke Blade engine secara global.
/// Contoh: `@directiveName` atau `@directiveName(argumen)`
pub fn register_global_directive<F>(name: &str, handler: F)
where
    F: Fn(&str, &BladeEngine, &BladeContext) -> String + Send + Sync + 'static,
{
    let lock = GLOBAL_DIRECTIVES.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()));
    let pattern_args = format!(r#"@{}\s*\((.*?)\)"#, regex::escape(name));
    let pattern_no_args = format!(r#"@{}\b"#, regex::escape(name));
    let re_args = Regex::new(&pattern_args).expect("Valid regex for custom directive with args");
    let re_no_args = Regex::new(&pattern_no_args).expect("Valid regex for custom directive without args");

    let mut map = lock.write().unwrap();
    map.insert(
        name.to_string(),
        RegisteredDirective {
            name: name.to_string(),
            re_args,
            re_no_args,
            handler: std::sync::Arc::new(handler),
        },
    );
}

impl BladeEngine {
    /// Membuat BladeEngine kosong (tanpa membaca views path, cocok untuk test/memory engine)
    pub fn empty() -> Self {
        Self {
            templates: HashMap::new(),
            plugin_runner: Arc::new(NoopPluginRunner),
            wasm_filters: HashMap::new(),
        }
    }

    pub fn new() -> Self {
        let mut engine = Self::empty();
        engine.reload_templates("resources/views");
        engine
    }

    /// Pasang plugin_runner kustom/global (misal dari PakemPluginManager)
    pub fn with_plugin_runner(mut self, runner: Arc<dyn BladePluginRunner>) -> Self {
        self.plugin_runner = runner;
        self
    }

    /// Daftarkan alias filter Blade kustom yang mengarah ke plugin WASM tertentu
    pub fn register_wasm_filter(&mut self, alias: &str, plugin_name: &str, func_name: &str) {
        self.wasm_filters.insert(alias.to_string(), (plugin_name.to_string(), func_name.to_string()));
    }



    pub fn reload_templates(&mut self, views_path: &str) {
        self.templates.clear();
        if Path::new(views_path).is_dir() {
            Self::collect_templates(views_path, "", &mut self.templates);
        }
    }

    fn collect_templates(base_path: &str, prefix: &str, out: &mut HashMap<String, String>) {
        let path = if prefix.is_empty() {
            base_path.to_string()
        } else {
            format!("{}/{}", base_path, prefix)
        };

        if let Ok(entries) = fs::read_dir(&path) {
            for entry in entries.flatten() {
                let file_path = entry.path();
                let file_name = entry.file_name().into_string().unwrap_or_default();

                if file_path.is_dir() {
                    let new_prefix = if prefix.is_empty() {
                        file_name
                    } else {
                        format!("{}/{}", prefix, file_name)
                    };
                    Self::collect_templates(base_path, &new_prefix, out);
                } else if file_name.ends_with(".blade.rs") || file_name.ends_with(".html") {
                    match fs::read_to_string(&file_path) {
                        Ok(content) => {
                            let template_name = if prefix.is_empty() {
                                file_name.clone()
                            } else {
                                format!("{}/{}", prefix, file_name)
                            };
                            let normalized = template_name.replace('\\', "/");
                            out.insert(normalized.clone(), content.clone());

                            if normalized.ends_with(".blade.rs") {
                                let without_ext = normalized.strip_suffix(".blade.rs").unwrap();
                                out.insert(without_ext.to_string(), content.clone());
                                out.insert(without_ext.replace('/', "."), content.clone());
                            } else if normalized.ends_with(".html") {
                                let without_ext = normalized.strip_suffix(".html").unwrap();
                                out.insert(without_ext.to_string(), content.clone());
                                out.insert(without_ext.replace('/', "."), content.clone());
                            }
                        }
                        Err(e) => {
                            eprintln!("Failed to read template {:?}: {}", file_path, e);
                        }
                    }
                }
            }
        }
    }

    /// Render template menggunakan nama file atau format dot-notation (misal: "auth.login" atau "layouts/app.blade.rs")
    pub fn render(&self, template_name: &str, context: &BladeContext) -> String {
        let clean = template_name.replace('\\', "/");
        let raw = self.templates.get(&clean)
            .or_else(|| {
                if !clean.ends_with(".blade.rs") && !clean.ends_with(".html") {
                    let with_blade = format!("{}.blade.rs", clean.replace('.', "/"));
                    self.templates.get(&with_blade)
                } else {
                    None
                }
            })
            .or_else(|| {
                let dotted = clean.replace('/', ".");
                self.templates.get(&dotted)
            });

        let raw = match raw {
            Some(content) => content.clone(),
            None => {
                let keys = self.templates.keys().cloned().collect::<Vec<_>>();
                return format!("Template error: Template '{}' not found. Available templates: {:?}", template_name, keys);
            }
        };

        self.process_template(&raw, context, 0)
    }

    /// Render string template secara langsung tanpa membaca file
    pub fn render_string(&self, content: &str, context: &BladeContext) -> String {
        self.process_template(content, context, 0)
    }

    /// Pipeline utama pemrosesan template Blade
    fn process_template(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        if depth > 25 {
            return "<!-- Template recursion limit exceeded -->".to_string();
        }

        let mut s = content.to_string();

        // 0. {{-- Comment --}}
        s = RE_COMMENT.replace_all(&s, "").to_string();

        // 0b. @once ... @endonce
        if s.contains("@once") {
            s = RE_ONCE.replace_all(&s, |caps: &regex::Captures| {
                let inner = &caps[1];
                let trimmed = inner.trim();
                if context.check_and_mark_once(trimmed) {
                    inner.to_string()
                } else {
                    String::new()
                }
            }).to_string();
        }

        // 1. Template Inheritance: @extends, @section, @yield, @hasSection
        s = self.process_inheritance(&s, context, depth);

        // 2. Includes: @include, @includeIf, @includeWhen, @includeUnless
        s = self.process_includes(&s, context, depth);

        // 2c. Blade Components: <x-component-name ... /> dan <x-component-name> ... </x-component-name>
        s = self.process_components(&s, context, depth);

        // 2b. Stacks: @push, @prepend, @stack
        if s.contains("@push") {
            for caps in RE_PUSH.captures_iter(&s) {
                let name = caps[1].trim();
                let body = caps[2].to_string();
                context.push_to_stack(name, body);
            }
            s = RE_PUSH.replace_all(&s, "").to_string();
        }
        if s.contains("@prepend") {
            for caps in RE_PREPEND.captures_iter(&s) {
                let name = caps[1].trim();
                let body = caps[2].to_string();
                context.prepend_to_stack(name, body);
            }
            s = RE_PREPEND.replace_all(&s, "").to_string();
        }
        if s.contains("@stack") {
            s = RE_STACK.replace_all(&s, |caps: &regex::Captures| {
                let name = caps[1].trim();
                context.get_stack(name)
            }).to_string();
        }

        // 3. Normalisasi direktif varian kondisional ke @if ... @endif standar
        s = self.normalize_conditionals(&s);

        // 4. Perulangan: @foreach, @forelse, @for
        s = self.process_loops(&s, context, depth);

        // 5. Kondisional bercabang: @if, @elseif, @else, @endif (mendukung nesting)
        s = self.process_if_blocks(&s, context, depth);

        // 6. Switch-Case: @switch, @case, @break, @default, @endswitch
        s = self.process_switch(&s, context, depth);

        // 7. Form & Security Helpers: @csrf, @method
        s = s.replace("@csrf", &format!("<input type=\"hidden\" name=\"csrf_token\" value=\"{}\">", context.get("csrf_token").and_then(|v| v.as_str()).unwrap_or("")));
        s = RE_METHOD.replace_all(&s, "<input type=\"hidden\" name=\"_method\" value=\"$1\">").to_string();

        // 7a. Asset Bundler: @vite(...)
        s = self.process_vite(&s);

        // 7b. Form Error Directives: @error('field') ... @enderror
        s = self.process_errors(&s, context, depth);

        // 8. Attribute Helpers: @checked, @selected, @disabled, @readonly, @required, @class, @style
        s = self.process_attributes(&s, context);

        // 9. Debug & Output Helpers: @json, @dump, @dd
        s = self.process_debug_helpers(&s, context);

        // 10. Indonesian Script: @is('script') / @script('script')
        s = self.process_indonesian_script(&s);

        // 11. Direktif WASM: @wasm('plugin_name', 'function_name', $arg)
        s = RE_WASM.replace_all(&s, |caps: &regex::Captures| {
            let plugin_name = &caps[1];
            let func_name = &caps[2];
            let arg = caps.get(3).map(|m| context.eval_val(m.as_str())).unwrap_or_default();
            match self.plugin_runner.call_filter(plugin_name, func_name, &arg) {
                Ok(out) => out,
                Err(e) => format!("<!-- WASM error: {} -->", e),
            }
        }).to_string();

        // 12. Direktif Evaluasi Backend Indonesian Script: @eval_is('file.is', 'func', $arg1, $arg2, ...)
        s = RE_EVAL_IS.replace_all(&s, |caps: &regex::Captures| {
            let script_path = &caps[1];
            let func_name = &caps[2];
            let mut args_vec = Vec::new();
            if let Some(args_raw) = caps.get(3) {
                for arg in args_raw.as_str().split(',') {
                    let clean_arg = arg.trim();
                    if !clean_arg.is_empty() {
                        let evaled = context.eval_val(clean_arg);
                        if let Ok(num) = evaled.parse::<f64>() {
                            args_vec.push(json!(num));
                        } else if evaled == "true" {
                            args_vec.push(json!(true));
                        } else if evaled == "false" {
                            args_vec.push(json!(false));
                        } else {
                            args_vec.push(json!(evaled));
                        }
                    }
                }
            }

            match self.plugin_runner.eval_is_script(script_path, func_name, &args_vec) {
                Ok(val) => match val {
                    Value::String(s) => s,
                    Value::Number(n) => {
                        if let Some(i) = n.as_i64() {
                            i.to_string()
                        } else if let Some(f) = n.as_f64() {
                            if f.fract() == 0.0 {
                                (f as i64).to_string()
                            } else {
                                f.to_string()
                            }
                        } else {
                            n.to_string()
                        }
                    },
                    Value::Bool(b) => b.to_string(),
                    Value::Null => String::new(),
                    other => other.to_string(),
                },
                Err(e) => format!("<!-- Indonesian Script error: {} -->", e),
            }
        }).to_string();

        // 13. Ekstensi Custom Directives (didukung oleh pakem-live atau plugin pihak ketiga - zero regex compilation)
        if s.contains('@') {
            if let Some(lock) = GLOBAL_DIRECTIVES.get() {
                let directives = lock.read().unwrap();
                for dir in directives.values() {
                    if !s.contains(&dir.name) {
                        continue;
                    }
                    s = dir.re_args.replace_all(&s, |caps: &regex::Captures| {
                        (dir.handler)(&caps[1], self, context)
                    }).to_string();

                    s = dir.re_no_args.replace_all(&s, |_caps: &regex::Captures| {
                        (dir.handler)("", self, context)
                    }).to_string();
                }
            }
        }
        // 15. Interpolasi Variabel & Filter: {!! raw !!} dan {{ escaped }}

        s = RE_RAW.replace_all(&s, |caps: &regex::Captures| {
            self.eval_expr_with_filters(&caps[1], context)
        }).to_string();

        s = RE_VAR.replace_all(&s, |caps: &regex::Captures| {
            html_escape::encode_safe(&self.eval_expr_with_filters(&caps[1], context))
        }).to_string();

        // 16. Production Template Shield (@productionShield & anti-inspect / anti-copas)
        s = self.process_production_shield(s, context, depth);

        s
    }

    /// Evaluasi ekspresi variabel atau literal yang mungkin memiliki filter berantai:
    /// Contoh: `$nominal | rupiah`
    /// Contoh: `$title | slug | uppercase`
    /// Contoh: `$content | limit: 100`
    /// Contoh: `$comment | sensor_kata` (WASM Sandbox)
    pub fn eval_expr_with_filters(&self, expr: &str, context: &BladeContext) -> String {
        let expr = expr.trim();
        if !expr.contains('|') {
            return context.eval_val(expr);
        }

        // Simpan operator '||' jika ada agar tidak terpotong pipa filter
        let safe_expr = expr.replace("||", "__LOGICAL_OR__");
        let parts: Vec<&str> = safe_expr.split('|').collect();

        let base_raw = parts[0].replace("__LOGICAL_OR__", "||");
        let mut current_val = context.eval_val(&base_raw);

        for segment in &parts[1..] {
            let seg = segment.trim();
            if seg.is_empty() {
                continue;
            }

            // Parsing filter name dan parameter opsional:
            // Format 1 (colon style): filter_name: arg1, arg2
            // Format 2 (paren style): filter_name(arg1, arg2)
            let (filter_name, args) = if let Some(colon_idx) = seg.find(':') {
                let name = seg[..colon_idx].trim();
                let raw_args = &seg[colon_idx + 1..];
                let args_vec: Vec<&str> = raw_args.split(',').map(|a| a.trim()).collect();
                (name, args_vec)
            } else if let Some(paren_idx) = seg.find('(') {
                let name = seg[..paren_idx].trim();
                let end_paren = seg.rfind(')').unwrap_or(seg.len());
                let raw_args = &seg[paren_idx + 1..end_paren];
                let args_vec: Vec<&str> = raw_args.split(',').map(|a| a.trim()).collect();
                (name, args_vec)
            } else {
                (seg, Vec::new())
            };

            current_val = self.apply_filter(filter_name, &args, &current_val);
        }

        current_val
    }

    /// Terapkan filter pada nilai string (Native Rust Filters + WASM Sandbox Fallback)
    fn apply_filter(&self, filter_name: &str, args: &[&str], input: &str) -> String {
        match filter_name {
            // Native Filter: Format Mata Uang Rupiah
            "rupiah" | "formatRupiah" => Self::format_rupiah(input),

            // Native Filter: URL Slug Generator
            "slug" => Self::slug(input),

            // Native Filter: Huruf Besar (Uppercase)
            "upper" | "uppercase" => input.to_uppercase(),

            // Native Filter: Huruf Kecil (Lowercase)
            "lower" | "lowercase" => input.to_lowercase(),

            // Native Filter: Kapitalisasi Kata
            "capitalize" | "title" => {
                let mut c = input.chars();
                match c.next() {
                    None => String::new(),
                    Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                }
            }

            // Native Filter: Pangkas Spasi
            "trim" => input.trim().to_string(),

            // Native Filter: Pemotongan Teks (limit: 50 atau potong: 50)
            "limit" | "potong" => {
                let max_len = args.first().and_then(|a| a.parse::<usize>().ok()).unwrap_or(100);
                if input.chars().count() > max_len {
                    let truncated: String = input.chars().take(max_len).collect();
                    format!("{}...", truncated)
                } else {
                    input.to_string()
                }
            }

            // Native Filter: Balik Karakter
            "reverse" | "balik" => input.chars().rev().collect(),

            // Native Filter: Inisial Huruf Pertama
            "initial" | "inisial" => {
                input.trim().chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
            }

            // Native Filter: Pemotongan String Substring
            "substr" | "substring" => {
                let start = args.first().and_then(|a| a.parse::<usize>().ok()).unwrap_or(0);
                let len = args.get(1).and_then(|a| a.parse::<usize>().ok());
                let chars: Vec<char> = input.chars().collect();
                if start >= chars.len() {
                    String::new()
                } else if let Some(l) = len {
                    chars.into_iter().skip(start).take(l).collect()
                } else {
                    chars.into_iter().skip(start).collect()
                }
            }

            // WASM Sandbox Fallback: Delegasikan ke WebAssembly Plugin
            custom_filter => {
                // 1. Cek pemetaan alias filter terdaftar
                if let Some((plugin_name, func_name)) = self.wasm_filters.get(custom_filter) {
                    if let Ok(res) = self.plugin_runner.call_filter(plugin_name, func_name, input) {
                        return res;
                    }
                }

                // 2. Cek apakah ada modul plugin dengan nama filter itu sendiri
                if let Ok(res) = self.plugin_runner.call_filter(custom_filter, "transform", input) {
                    return res;
                }
                if let Ok(res) = self.plugin_runner.call_filter(custom_filter, custom_filter, input) {
                    return res;
                }

                // Jika filter tidak ditemukan, kembalikan teks asli dengan aman tanpa panic
                input.to_string()
            }
        }
    }

    /// Helper Native: Format angka ke format mata uang Rupiah
    pub fn format_rupiah(input: &str) -> String {
        let clean = input.trim().replace(',', "");
        if let Ok(num) = clean.parse::<f64>() {
            let int_part = num.trunc() as i64;
            let s = int_part.abs().to_string();
            let mut formatted = String::new();
            let mut count = 0;
            for c in s.chars().rev() {
                if count > 0 && count % 3 == 0 {
                    formatted.push('.');
                }
                formatted.push(c);
                count += 1;
            }
            let reversed: String = formatted.chars().rev().collect();
            if int_part < 0 {
                format!("-Rp {}", reversed)
            } else {
                format!("Rp {}", reversed)
            }
        } else {
            format!("Rp {}", input)
        }
    }

    /// Helper Native: Mengubah teks menjadi slug URL (contoh: "Halo Dunia!" -> "halo-dunia")
    pub fn slug(input: &str) -> String {
        let mut result = String::new();
        let mut last_was_dash = false;
        for c in input.chars() {
            if c.is_alphanumeric() {
                result.push(c.to_ascii_lowercase());
                last_was_dash = false;
            } else if !last_was_dash && !result.is_empty() {
                result.push('-');
                last_was_dash = true;
            }
        }
        if result.ends_with('-') {
            result.pop();
        }
        result
    }


    /// Normalisasi sintaks kondisional Laravel menjadi format `@if` standar
    fn normalize_conditionals(&self, input: &str) -> String {
        let mut s = input.to_string();

        // @unless(expr) -> @if(!(expr)) & @endunless -> @endif
        s = RE_UNLESS.replace_all(&s, "@if(!($1))").to_string();
        s = s.replace("@endunless", "@endif");

        // @isset(var) -> @if(isset(var)) & @endisset -> @endif
        s = RE_ISSET.replace_all(&s, "@if(isset($1))").to_string();
        s = s.replace("@endisset", "@endif");

        // @empty(var) -> @if(empty(var)) & @endempty -> @endif
        s = RE_EMPTY.replace_all(&s, "@if(empty($1))").to_string();
        s = s.replace("@endempty", "@endif");

        // @auth -> @if(auth) & @endauth -> @endif
        s = RE_AUTH.replace_all(&s, "@if(auth)").to_string();
        s = s.replace("@endauth", "@endif");

        // @guest -> @if(guest) & @endguest -> @endif
        s = RE_GUEST.replace_all(&s, "@if(guest)").to_string();
        s = s.replace("@endguest", "@endif");

        // @can('permission') -> @if(can('permission')) & @endcan -> @endif
        s = RE_CAN.replace_all(&s, "@if(can($1))").to_string();
        s = s.replace("@endcan", "@endif");

        // @cannot('permission') -> @if(cannot($1)) & @endcannot -> @endif
        s = RE_CANNOT.replace_all(&s, "@if(cannot($1))").to_string();
        s = s.replace("@endcannot", "@endif");

        // @role('role') -> @if(role($1)) & @endrole -> @endif
        s = RE_ROLE.replace_all(&s, "@if(role($1))").to_string();
        s = s.replace("@endrole", "@endif");

        // @production -> @if(production) & @endproduction -> @endif
        s = RE_PROD.replace_all(&s, "@if(production)").to_string();
        s = s.replace("@endproduction", "@endif");

        // @env(...) -> @if(env(...)) & @endenv -> @endif
        s = RE_ENV.replace_all(&s, "@if(env($1))").to_string();
        s = s.replace("@endenv", "@endif");

        s
    }

    /// Penanganan pewarisan template (@extends, @section, @yield)
    fn process_inheritance(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        let mut s = content.to_string();

        
        if let Some(caps) = RE_EXTENDS.captures(&s) {
            let layout_raw = caps[1].trim().replace('\\', "/");
            let layout_content = self.templates.get(&layout_raw)
                .or_else(|| {
                    let normalized = layout_raw.replace('.', "/");
                    let file = if normalized.ends_with(".blade.rs") || normalized.ends_with(".html") {
                        normalized
                    } else {
                        format!("{}.blade.rs", normalized)
                    };
                    self.templates.get(&file)
                });

            let mut sections: HashMap<String, String> = HashMap::new();

            // Short inline syntax: @section('title', 'Page Title')
            
            for sec_caps in RE_SHORT_SECTION.captures_iter(&s) {
                sections.insert(sec_caps[1].to_string(), sec_caps[2].to_string());
            }

            // Block syntax: @section('name') ... @endsection / @stop / @show
            
            for sec_caps in RE_SECTION.captures_iter(&s) {
                let sec_name = sec_caps[1].to_string();
                let sec_content = sec_caps[2].to_string();
                sections.insert(sec_name, sec_content);
            }

            // Extract @push in child template
            for push_caps in RE_PUSH.captures_iter(&s) {
                let name = push_caps[1].trim();
                let body = push_caps[2].to_string();
                context.push_to_stack(name, body);
            }
            s = RE_PUSH.replace_all(&s, "").to_string();

            // Extract @prepend in child template
            for prep_caps in RE_PREPEND.captures_iter(&s) {
                let name = prep_caps[1].trim();
                let body = prep_caps[2].to_string();
                context.prepend_to_stack(name, body);
            }
            s = RE_PREPEND.replace_all(&s, "").to_string();

            // Load parent layout
            if let Some(parent_content) = layout_content {
                let mut layout_processed = parent_content.clone();

                // 1. @yield('name', 'default fallback')
                layout_processed = RE_YIELD_DEFAULT.replace_all(&layout_processed, |y_caps: &regex::Captures| {
                    let y_name = &y_caps[1];
                    let y_default = &y_caps[2];
                    sections.get(y_name).cloned().unwrap_or_else(|| y_default.to_string())
                }).to_string();

                // 2. @yield('name')
                layout_processed = RE_YIELD.replace_all(&layout_processed, |y_caps: &regex::Captures| {
                    let y_name = &y_caps[1];
                    sections.get(y_name).cloned().unwrap_or_default()
                }).to_string();

                // 3. @hasSection('name')
                layout_processed = RE_HAS_SEC.replace_all(&layout_processed, |h_caps: &regex::Captures| {
                    if sections.contains_key(&h_caps[1]) {
                        "@if(true)".to_string()
                    } else {
                        "@if(false)".to_string()
                    }
                }).to_string();

                s = self.process_template(&layout_processed, context, depth + 1);
            }
        }

        s
    }

    /// Penanganan @include, @includeIf, @includeWhen, @includeUnless (mendukung parameter data array)
    fn process_includes(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        let mut s = content.to_string();

        // @includeWhen(cond, 'path', ['var' => $val])
        s = RE_INC_WHEN.replace_all(&s, |caps: &regex::Captures| {
            let inner = &caps[1];
            let args = BladeContext::split_arguments(inner);
            if args.len() >= 2 {
                let cond = &args[0];
                let path = args[1].trim().trim_matches('\'').trim_matches('"');
                if context.eval_truthy(cond) {
                    let mut sub_ctx = context.clone();
                    if args.len() >= 3 {
                        let extra = BladeContext::parse_blade_array(&args[2], context);
                        for (k, v) in extra {
                            sub_ctx.insert(&k, v);
                        }
                    }
                    self.resolve_include(path, &sub_ctx, depth, false)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        }).to_string();

        // @includeUnless(cond, 'path', ['var' => $val])
        s = RE_INC_UNLESS.replace_all(&s, |caps: &regex::Captures| {
            let inner = &caps[1];
            let args = BladeContext::split_arguments(inner);
            if args.len() >= 2 {
                let cond = &args[0];
                let path = args[1].trim().trim_matches('\'').trim_matches('"');
                if !context.eval_truthy(cond) {
                    let mut sub_ctx = context.clone();
                    if args.len() >= 3 {
                        let extra = BladeContext::parse_blade_array(&args[2], context);
                        for (k, v) in extra {
                            sub_ctx.insert(&k, v);
                        }
                    }
                    self.resolve_include(path, &sub_ctx, depth, false)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        }).to_string();

        // @includeIf('path', ['var' => $val])
        s = RE_INC_IF.replace_all(&s, |caps: &regex::Captures| {
            let inner = &caps[1];
            let args = BladeContext::split_arguments(inner);
            if !args.is_empty() {
                let path = args[0].trim().trim_matches('\'').trim_matches('"');
                let mut sub_ctx = context.clone();
                if args.len() >= 2 {
                    let extra = BladeContext::parse_blade_array(&args[1], context);
                    for (k, v) in extra {
                        sub_ctx.insert(&k, v);
                    }
                }
                self.resolve_include(path, &sub_ctx, depth, true)
            } else {
                String::new()
            }
        }).to_string();

        // @include('path', ['var' => $val])
        s = RE_INC.replace_all(&s, |caps: &regex::Captures| {
            let inner = &caps[1];
            let args = BladeContext::split_arguments(inner);
            if !args.is_empty() {
                let path = args[0].trim().trim_matches('\'').trim_matches('"');
                let mut sub_ctx = context.clone();
                if args.len() >= 2 {
                    let extra = BladeContext::parse_blade_array(&args[1], context);
                    for (k, v) in extra {
                        sub_ctx.insert(&k, v);
                    }
                }
                self.resolve_include(path, &sub_ctx, depth, false)
            } else {
                String::new()
            }
        }).to_string();

        s
    }

    fn resolve_include(&self, path_raw: &str, context: &BladeContext, depth: usize, silent: bool) -> String {
        let clean = path_raw.trim().replace('\\', "/");
        let inc_content = self.templates.get(&clean)
            .or_else(|| {
                let normalized = clean.replace('.', "/");
                let file = if normalized.ends_with(".blade.rs") || normalized.ends_with(".html") {
                    normalized
                } else {
                    format!("{}.blade.rs", normalized)
                };
                self.templates.get(&file)
            });

        if let Some(content) = inc_content {
            self.process_template(content, context, depth + 1)
        } else if silent {
            String::new()
        } else {
            format!("<!-- Include '{}' not found -->", path_raw)
        }
    }

    fn render_component_tag(&self, tag_name: &str, attr_str: &str, body_opt: Option<&str>, context: &BladeContext, depth: usize) -> String {
        let clean_name = tag_name.replace('.', "/");
        let candidates = [
            format!("components/{}.blade.rs", clean_name),
            format!("components/{}.blade.rs", clean_name.replace('-', "_")),
            format!("components/{}.blade.rs", clean_name.replace('-', "/")),
            format!("components/{}.html", clean_name),
            format!("{}.blade.rs", clean_name),
        ];

        let mut comp_content = None;
        for cand in &candidates {
            if let Some(tmpl) = self.templates.get(cand) {
                comp_content = Some(tmpl.clone());
                break;
            }
        }

        if let Some(comp_template) = comp_content {
            let mut sub_ctx = context.clone();

            // 1. Parse attributes
            for attr_cap in RE_COMPONENT_ATTR.captures_iter(attr_str) {
                if let Some(bound_name) = attr_cap.get(1) {
                    let val_expr = attr_cap.get(3).or_else(|| attr_cap.get(4)).map(|m| m.as_str()).unwrap_or("");
                    let clean_expr = val_expr.trim_start_matches('$');
                    if let Some(v) = context.get(clean_expr) {
                        sub_ctx.insert(bound_name.as_str(), v.clone());
                    } else {
                        let evaled = context.eval_val(val_expr);
                        sub_ctx.insert(bound_name.as_str(), evaled);
                    }
                } else if let Some(plain_name) = attr_cap.get(2) {
                    let val = attr_cap.get(3).or_else(|| attr_cap.get(4)).map(|m| m.as_str()).unwrap_or("");
                    sub_ctx.insert(plain_name.as_str(), val);
                }
            }

            // 2. Parse slots jika berbentuk block tag
            if let Some(body) = body_opt {
                let mut slot_body = body.to_string();
                for slot_cap in RE_COMPONENT_SLOT.captures_iter(body) {
                    let slot_name = slot_cap.get(1).or_else(|| slot_cap.get(2)).map(|m| m.as_str()).unwrap_or("slot");
                    let slot_html = &slot_cap[3];
                    let rendered_slot = self.process_template(slot_html, context, depth + 1);
                    sub_ctx.insert(slot_name, rendered_slot);
                }
                slot_body = RE_COMPONENT_SLOT.replace_all(&slot_body, "").to_string();
                let rendered_default_slot = self.process_template(slot_body.trim(), context, depth + 1);
                sub_ctx.insert("slot", rendered_default_slot.clone());
                sub_ctx.insert("$slot", rendered_default_slot);
            }

            self.process_template(&comp_template, &sub_ctx, depth + 1)
        } else {
            if let Some(body) = body_opt {
                format!("<x-{}{}>{}</x-{}>", tag_name, attr_str, body, tag_name)
            } else {
                format!("<x-{}{}/>", tag_name, attr_str)
            }
        }
    }

    /// Penanganan Blade Component modern: <x-alert type="warning" :count="$total">Slot Body</x-alert>
    fn process_components(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        if !content.contains("<x-") {
            return content.to_string();
        }

        // 1. Self-closing components: <x-alert ... />
        let mut s = RE_COMPONENT_SELF.replace_all(content, |caps: &regex::Captures| {
            let tag_name = &caps[1];
            let attr_str = &caps[2];
            self.render_component_tag(tag_name, attr_str, None, context, depth)
        }).to_string();

        // 2. Block components: <x-card ...> ... </x-card>
        loop {
            if let Some(m) = RE_COMPONENT_OPEN.find(&s) {
                let caps = RE_COMPONENT_OPEN.captures(&s[m.start()..]).unwrap();
                let tag_name = caps[1].to_string();
                let attr_str = caps[2].to_string();
                let open_len = caps[0].len();
                let body_start = m.start() + open_len;

                let close_tag = format!("</x-{}>", tag_name);
                if let Some(close_idx) = s[body_start..].find(&close_tag) {
                    let body = s[body_start..body_start + close_idx].to_string();
                    let rendered = self.render_component_tag(&tag_name, &attr_str, Some(&body), context, depth);
                    s.replace_range(m.start()..body_start + close_idx + close_tag.len(), &rendered);
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        s
    }

    /// Penanganan asset bundler Laravel @vite(['resources/css/app.css', 'resources/js/app.js'])
    fn process_vite(&self, content: &str) -> String {
        if !content.contains("@vite") {
            return content.to_string();
        }

        RE_VITE.replace_all(content, |caps: &regex::Captures| {
            let inner = caps[1].trim();
            let items: Vec<String> = if inner.starts_with('[') && inner.ends_with(']') {
                let inside = &inner[1..inner.len() - 1];
                BladeContext::split_arguments(inside)
            } else {
                vec![inner.to_string()]
            };

            let mut tags = Vec::new();
            for item in items {
                let path_raw = item.trim().trim_matches('\'').trim_matches('"');
                if path_raw.is_empty() {
                    continue;
                }
                let clean_path = if path_raw.starts_with("resources/") {
                    format!("/{}", &path_raw[10..])
                } else if path_raw.starts_with('/') {
                    path_raw.to_string()
                } else {
                    format!("/{}", path_raw)
                };

                if clean_path.ends_with(".css") {
                    tags.push(format!(r#"<link rel="stylesheet" href="{}">"#, clean_path));
                } else {
                    tags.push(format!(r#"<script type="module" src="{}"></script>"#, clean_path));
                }
            }
            tags.join("\n")
        }).to_string()
    }

    /// HTML, CSS, dan JavaScript untuk proteksi template di browser (Anti-DevTools, Anti-Inspect, Anti-Copy)
    pub fn production_shield_html() -> &'static str {
        r#"<style id="_pakem_shield_css">
html,body{-webkit-user-select:none!important;-moz-user-select:none!important;-ms-user-select:none!important;user-select:none!important;-webkit-touch-callout:none!important}
img{pointer-events:none!important;-webkit-user-drag:none!important}
</style>
<script id="_pakem_shield_js">
(function(){
document.addEventListener("contextmenu",function(e){e.preventDefault();return false;});
document.addEventListener("keydown",function(e){
if(e.keyCode===123||(e.ctrlKey&&e.keyCode===85)||(e.ctrlKey&&e.keyCode===83)||(e.ctrlKey&&e.shiftKey&&(e.keyCode===73||e.keyCode===74||e.keyCode===67||e.keyCode===75))){
e.preventDefault();e.stopPropagation();return false;
}
});
setInterval(function(){(function(){return false;}["constructor"]("debugger")());},100);
var _dt=false;
setInterval(function(){
if((window.outerWidth-window.innerWidth>160)||(window.outerHeight-window.innerHeight>160)){
if(!_dt){_dt=true;document.body.innerHTML='<div style="display:flex;justify-content:center;align-items:center;height:100vh;background:#0f172a;color:#ef4444;font-family:sans-serif;font-size:24px;font-weight:700;">Akses Terproteksi (DevTools Dinonaktifkan)</div>';}
}
},500);
})();
</script>"#
    }

    /// Potong komentar JS satu baris (`// ...`) yang berada di luar string/URL agar aman saat one-line minify
    fn strip_inline_js_comment(line: &str) -> &str {
        let bytes = line.as_bytes();
        let mut in_single = false;
        let mut in_double = false;
        let mut in_backtick = false;
        let mut escaped = false;
        let mut i = 0;
        while i + 1 < bytes.len() {
            let b = bytes[i];
            if escaped {
                escaped = false;
                i += 1;
                continue;
            }
            if b == b'\\' && (in_single || in_double || in_backtick) {
                escaped = true;
                i += 1;
                continue;
            }
            if b == b'\'' && !in_double && !in_backtick {
                in_single = !in_single;
            } else if b == b'"' && !in_single && !in_backtick {
                in_double = !in_double;
            } else if b == b'`' && !in_single && !in_double {
                in_backtick = !in_backtick;
            } else if !in_single && !in_double && !in_backtick && b == b'/' && bytes[i + 1] == b'/' {
                if i == 0 || bytes[i - 1] != b':' {
                    return line[..i].trim_end();
                }
            }
            i += 1;
        }
        line
    }

    /// Helper minifikasi dokumen HTML (menghilangkan baris kosong, komentar, dan memadatkan whitespace menjadi 1 baris)
    pub fn minify_html(input: &str) -> String {
        let mut out = String::with_capacity(input.len());
        let mut prev_space = false;
        for line in input.lines() {
            let trimmed = Self::strip_inline_js_comment(line.trim());
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
                continue;
            }
            if !out.is_empty() && !prev_space {
                out.push(' ');
            }
            out.push_str(trimmed);
            prev_space = trimmed.ends_with('>') || trimmed.ends_with(';');
        }
        out
    }

    /// Penanganan proteksi template production dan minifikasi output HTML
    fn process_production_shield(&self, mut s: String, context: &BladeContext, depth: usize) -> String {
        let shield_requested = s.contains("@productionShield")
            || context.get("__production_shield").and_then(|v| v.as_bool()).unwrap_or(false)
            || std::env::var("BLADE_PRODUCTION_SHIELD").map(|v| v == "true" || v == "1").unwrap_or(false);

        if s.contains("@productionShield") {
            s = s.replace("@productionShield", Self::production_shield_html());
        } else if shield_requested && depth == 0 {
            if s.contains("</body>") {
                s = s.replace("</body>", &format!("{}\n</body>", Self::production_shield_html()));
            } else {
                s.push_str("\n");
                s.push_str(Self::production_shield_html());
            }
        }

        let minify_requested = context.get("__minify_html").and_then(|v| v.as_bool()).unwrap_or(false)
            || std::env::var("BLADE_MINIFY_HTML").map(|v| v == "true" || v == "1").unwrap_or(false);

        if minify_requested && depth == 0 {
            s = Self::minify_html(&s);
        }

        s
    }

    /// Penanganan perulangan (@foreach, @forelse, @for)
    fn process_loops(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        let mut s = content.to_string();

        loop {
            let m_foreach = RE_FOREACH_OPEN.find(&s);
            let m_forelse = RE_FORELSE_OPEN.find(&s);
            let m_for = RE_FOR_OPEN.find(&s);

            let mut candidates: Vec<(&str, usize, usize)> = Vec::new();
            if let Some(m) = m_foreach {
                candidates.push(("foreach", m.start(), m.end()));
            }
            if let Some(m) = m_forelse {
                candidates.push(("forelse", m.start(), m.end()));
            }
            if let Some(m) = m_for {
                candidates.push(("for", m.start(), m.end()));
            }

            if candidates.is_empty() {
                break;
            }

            candidates.sort_by_key(|c| c.1);
            let earliest = candidates[0];

            match earliest.0 {
                "foreach" => {
                    let start = earliest.1;
                    let header = match RE_FOREACH_OPEN.captures(&s[start..]) {
                        Some(caps) => caps[1].to_string(),
                        None => break,
                    };
                    let body_start = earliest.2;

                    // Cari matching @endforeach
                    let mut nesting = 1;
                    let mut idx = body_start;
                    let mut block_end = None;

                    while idx < s.len() {
                        let slice = &s[idx..];
                        if slice.starts_with("@foreach") {
                            nesting += 1;
                            idx += 8;
                        } else if slice.starts_with("@endforeach") {
                            nesting -= 1;
                            if nesting == 0 {
                                block_end = Some(idx);
                                break;
                            }
                            idx += 11;
                        } else {
                            idx += slice.chars().next().map_or(1, |c| c.len_utf8());
                        }
                    }

                    if let Some(end) = block_end {
                        let body = &s[body_start..end];
                        let rendered = self.render_foreach_loop(&header, body, "", context, depth);
                        s.replace_range(start..end + "@endforeach".len(), &rendered);
                    } else {
                        break;
                    }
                }
                "forelse" => {
                    let start = earliest.1;
                    let header = match RE_FORELSE_OPEN.captures(&s[start..]) {
                        Some(caps) => caps[1].to_string(),
                        None => break,
                    };
                    let body_start = earliest.2;

                    let mut nesting = 1;
                    let mut idx = body_start;
                    let mut empty_pos = None;
                    let mut block_end = None;

                    while idx < s.len() {
                        let slice = &s[idx..];
                        if slice.starts_with("@forelse") {
                            nesting += 1;
                            idx += 8;
                        } else if slice.starts_with("@empty") && nesting == 1 && (slice[6..].starts_with(|c: char| c.is_whitespace()) || slice.len() == 6) {
                            empty_pos = Some(idx);
                            idx += 6;
                        } else if slice.starts_with("@endforelse") {
                            nesting -= 1;
                            if nesting == 0 {
                                block_end = Some(idx);
                                break;
                            }
                            idx += 11;
                        } else {
                            idx += slice.chars().next().map_or(1, |c| c.len_utf8());
                        }
                    }

                    if let Some(end) = block_end {
                        let (body, empty_body) = if let Some(e_idx) = empty_pos {
                            (&s[body_start..e_idx], &s[e_idx + 6..end])
                        } else {
                            (&s[body_start..end], "")
                        };

                        let rendered = self.render_foreach_loop(&header, body, empty_body, context, depth);
                        s.replace_range(start..end + "@endforelse".len(), &rendered);
                    } else {
                        break;
                    }
                }
                "for" => {
                    let start = earliest.1;
                    let header = match RE_FOR_OPEN.captures(&s[start..]) {
                        Some(caps) => caps[1].to_string(),
                        None => break,
                    };
                    let body_start = earliest.2;

                    let mut nesting = 1;
                    let mut idx = body_start;
                    let mut block_end = None;

                    while idx < s.len() {
                        let slice = &s[idx..];
                        if slice.starts_with("@for") && !slice.starts_with("@foreach") && !slice.starts_with("@forelse") {
                            nesting += 1;
                            idx += 4;
                        } else if slice.starts_with("@endfor") && !slice.starts_with("@endforeach") && !slice.starts_with("@endforelse") {
                            nesting -= 1;
                            if nesting == 0 {
                                block_end = Some(idx);
                                break;
                            }
                            idx += 7;
                        } else {
                            idx += slice.chars().next().map_or(1, |c| c.len_utf8());
                        }
                    }

                    if let Some(end) = block_end {
                        let body = &s[body_start..end];
                        let rendered = self.render_for_loop(&header, body, context, depth);
                        s.replace_range(start..end + "@endfor".len(), &rendered);
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }

        s
    }

    fn render_foreach_loop(&self, header: &str, body: &str, empty_body: &str, context: &BladeContext, depth: usize) -> String {
        // Syntax: $collection as $item ATAU $collection as $key => $item
        let parts: Vec<&str> = header.split(" as ").collect();
        if parts.len() != 2 {
            return String::new();
        }

        let coll_expr = parts[0].trim();
        let target = parts[1].trim();

        let (key_var, item_var) = if let Some(arrow_idx) = target.find("=>") {
            (
                Some(target[..arrow_idx].trim().trim_start_matches('$').to_string()),
                target[arrow_idx + 2..].trim().trim_start_matches('$').to_string(),
            )
        } else {
            (None, target.trim_start_matches('$').to_string())
        };

        let collection_val = context.get(coll_expr);

        match collection_val {
            Some(Value::Array(items)) if !items.is_empty() => {
                let total = items.len();
                let mut out = String::new();
                for (idx, item) in items.iter().enumerate() {
                    let mut iter_ctx = context.clone();
                    iter_ctx.insert_val(&item_var, item.clone());
                    if let Some(ref k) = key_var {
                        iter_ctx.insert_val(k, json!(idx));
                    }

                    let loop_obj = json!({
                        "index": idx,
                        "iteration": idx + 1,
                        "remaining": total - (idx + 1),
                        "count": total,
                        "first": idx == 0,
                        "last": idx == total - 1,
                        "even": (idx + 1) % 2 == 0,
                        "odd": (idx + 1) % 2 != 0,
                        "depth": depth + 1,
                    });
                    iter_ctx.insert_val("loop", loop_obj);
                    out.push_str(&self.process_template(body, &iter_ctx, depth + 1));
                }
                out
            }
            Some(Value::Object(map)) if !map.is_empty() => {
                let total = map.len();
                let mut out = String::new();
                for (idx, (k, v)) in map.iter().enumerate() {
                    let mut iter_ctx = context.clone();
                    iter_ctx.insert_val(&item_var, v.clone());
                    if let Some(ref kv) = key_var {
                        iter_ctx.insert_val(kv, json!(k));
                    }

                    let loop_obj = json!({
                        "index": idx,
                        "iteration": idx + 1,
                        "remaining": total - (idx + 1),
                        "count": total,
                        "first": idx == 0,
                        "last": idx == total - 1,
                        "even": (idx + 1) % 2 == 0,
                        "odd": (idx + 1) % 2 != 0,
                        "depth": depth + 1,
                    });
                    iter_ctx.insert_val("loop", loop_obj);
                    out.push_str(&self.process_template(body, &iter_ctx, depth + 1));
                }
                out
            }
            _ => {
                if !empty_body.is_empty() {
                    self.process_template(empty_body, context, depth + 1)
                } else {
                    String::new()
                }
            }
        }
    }

    fn render_for_loop(&self, header: &str, body: &str, context: &BladeContext, depth: usize) -> String {
        // Support: $i in 0..5
        if header.contains(" in ") {
            let parts: Vec<&str> = header.split(" in ").collect();
            if parts.len() == 2 {
                let var = parts[0].trim().trim_start_matches('$');
                let range_parts: Vec<&str> = parts[1].split("..").collect();
                if range_parts.len() == 2 {
                    let start: i64 = range_parts[0].trim().parse().unwrap_or(0);
                    let end: i64 = range_parts[1].trim().parse().unwrap_or(0);
                    let mut out = String::new();
                    for i in start..end {
                        let mut iter_ctx = context.clone();
                        iter_ctx.insert_val(var, json!(i));
                        out.push_str(&self.process_template(body, &iter_ctx, depth + 1));
                    }
                    return out;
                }
            }
        }

        // Support standard C/PHP style: $i = 0; $i < 5; $i++
        let parts: Vec<&str> = header.split(';').collect();
        if parts.len() == 3 {
            
            
            if let (Some(init_caps), Some(cond_caps)) = (RE_INIT.captures(parts[0]), RE_COND.captures(parts[1])) {
                let var_name = &init_caps[1];
                let start: i64 = init_caps[2].parse().unwrap_or(0);
                let end: i64 = cond_caps[1].parse().unwrap_or(0);

                let mut out = String::new();
                for i in start..end {
                    let mut iter_ctx = context.clone();
                    iter_ctx.insert_val(var_name, json!(i));
                    out.push_str(&self.process_template(body, &iter_ctx, depth + 1));
                }
                return out;
            }
        }

        String::new()
    }

    fn extract_balanced_parens<'a>(s: &'a str, open_idx: usize) -> Option<(&'a str, usize)> {
        let mut depth = 0;
        let bytes = s.as_bytes();
        for i in open_idx..s.len() {
            if bytes[i] == b'(' {
                depth += 1;
            } else if bytes[i] == b')' {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[open_idx + 1..i], i + 1));
                }
            }
        }
        None
    }

    /// Penanganan percabangan @if ... @elseif ... @else ... @endif (mendukung nesting)
    fn process_if_blocks(&self, input: &str, context: &BladeContext, depth: usize) -> String {
        let mut result = input.to_string();

        while let Some(if_idx) = result.find("@if") {
            let after_if = &result[if_idx + 3..];
            let paren_rel = after_if.find('(');
            if paren_rel.is_none() || !after_if[..paren_rel.unwrap()].chars().all(|c| c.is_whitespace()) {
                break;
            }
            let open_paren = if_idx + 3 + paren_rel.unwrap();
            let (cond, after_if_end) = match Self::extract_balanced_parens(&result, open_paren) {
                Some(res) => (res.0.trim().to_string(), res.1),
                None => break,
            };

            let start_pos = if_idx;
            let mut branches: Vec<(Option<String>, usize, usize)> = Vec::new();
            let mut curr_cond = Some(cond);
            let mut curr_branch_start = after_if_end;
            let mut nesting = 1;
            let mut block_end = None;

            let mut idx = after_if_end;

            while idx < result.len() {
                if result.as_bytes()[idx] == b'@' {
                    let slice = &result[idx..];
                    if slice.starts_with("@if") && slice[3..].starts_with(|c: char| c.is_whitespace() || c == '(') {
                        nesting += 1;
                        idx += 3;
                        continue;
                    } else if slice.starts_with("@endif") {
                        nesting -= 1;
                        if nesting == 0 {
                            branches.push((curr_cond, curr_branch_start, idx));
                            block_end = Some(idx + "@endif".len());
                            break;
                        }
                        idx += 6;
                        continue;
                    } else if nesting == 1 {
                        if slice.starts_with("@elseif") {
                            let after_elseif = &slice[7..];
                            if let Some(p) = after_elseif.find('(') {
                                if after_elseif[..p].chars().all(|c| c.is_whitespace()) {
                                    if let Some((elseif_cond, after_paren)) = Self::extract_balanced_parens(slice, 7 + p) {
                                        branches.push((curr_cond, curr_branch_start, idx));
                                        curr_cond = Some(elseif_cond.trim().to_string());
                                        idx += after_paren;
                                        curr_branch_start = idx;
                                        continue;
                                    }
                                }
                            }
                        } else if slice.starts_with("@else") && !slice[5..].starts_with(|c: char| c.is_alphanumeric() || c == '_') {
                            branches.push((curr_cond, curr_branch_start, idx));
                            curr_cond = None;
                            idx += 5;
                            curr_branch_start = idx;
                            continue;
                        }
                    }
                }
                idx += result[idx..].chars().next().map_or(1, |c| c.len_utf8());
            }

            if let Some(end_pos) = block_end {
                let mut chosen_content = String::new();
                for (branch_cond, b_start, b_end) in branches {
                    let is_match = match branch_cond {
                        Some(ref c) => context.eval_truthy(c),
                        None => true,
                    };
                    if is_match {
                        chosen_content = result[b_start..b_end].to_string();
                        break;
                    }
                }

                let rendered_branch = self.process_template(&chosen_content, context, depth + 1);
                result.replace_range(start_pos..end_pos, &rendered_branch);
            } else {
                break;
            }
        }

        result
    }

    /// Penanganan @switch ... @case ... @default ... @endswitch
    fn process_switch(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        let mut s = content.to_string();
        s = RE_SWITCH.replace_all(&s, |caps: &regex::Captures| {
            let target_val = context.eval_val(&caps[1]);
            let body = &caps[2];

            
            let mut matched = false;
            let mut output = String::new();

            for case_caps in RE_CASES.captures_iter(body) {
                let case_val = case_caps[1].trim();
                let case_body = case_caps[2].trim();
                if case_val == target_val {
                    matched = true;
                    output = self.process_template(case_body, context, depth + 1);
                    break;
                }
            }

            if !matched {
                
                if let Some(def_caps) = RE_DEFAULT.captures(body) {
                    output = self.process_template(def_caps[1].trim(), context, depth + 1);
                }
            }

            output
        }).to_string();

        s
    }

    /// Penanganan direktif error validasi form: @error('field') ... @enderror
    fn process_errors(&self, content: &str, context: &BladeContext, depth: usize) -> String {
        RE_ERROR.replace_all(content, |caps: &regex::Captures| {
            let field = &caps[1];
            let inner = &caps[2];

            if let Some(msg) = context.get_error(field) {
                let mut sub_ctx = context.clone();
                sub_ctx.insert("message", &msg);
                sub_ctx.insert("$message", &msg);
                let replaced = inner
                    .replace("{{ $message }}", &html_escape::encode_safe(&msg))
                    .replace("{{$message}}", &html_escape::encode_safe(&msg))
                    .replace("{{ $message}}", &html_escape::encode_safe(&msg))
                    .replace("{{$message }}", &html_escape::encode_safe(&msg))
                    .replace("{{ message }}", &html_escape::encode_safe(&msg))
                    .replace("{{message}}", &html_escape::encode_safe(&msg))
                    .replace("{!! $message !!}", &msg)
                    .replace("{!!$message!!}", &msg);
                self.process_template(&replaced, &sub_ctx, depth + 1)
            } else {
                String::new()
            }
        }).to_string()
    }

    /// Penanganan direktif atribut HTML: @checked, @selected, @disabled, @readonly, @required, @class, @style
    fn process_attributes(&self, content: &str, context: &BladeContext) -> String {
        let mut s = content.to_string();

        // @checked(cond)
        s = RE_CHECKED.replace_all(&s, |caps: &regex::Captures| {
            if context.eval_truthy(&caps[1]) { "checked" } else { "" }
        }).to_string();

        // @selected(cond)
        s = RE_SELECTED.replace_all(&s, |caps: &regex::Captures| {
            if context.eval_truthy(&caps[1]) { "selected" } else { "" }
        }).to_string();

        // @disabled(cond)
        s = RE_DISABLED.replace_all(&s, |caps: &regex::Captures| {
            if context.eval_truthy(&caps[1]) { "disabled" } else { "" }
        }).to_string();

        // @readonly(cond)
        s = RE_READONLY.replace_all(&s, |caps: &regex::Captures| {
            if context.eval_truthy(&caps[1]) { "readonly" } else { "" }
        }).to_string();

        // @required(cond)
        s = RE_REQUIRED.replace_all(&s, |caps: &regex::Captures| {
            if context.eval_truthy(&caps[1]) { "required" } else { "" }
        }).to_string();

        // @class(['p-4', 'font-bold' => $active, 'hidden' => !$show])
        s = RE_CLASS.replace_all(&s, |caps: &regex::Captures| {
            let inner = &caps[1];
            let mut active_classes: Vec<String> = Vec::new();
            for item in inner.split(',') {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                if let Some(arrow_idx) = item.find("=>") {
                    let class_name = item[..arrow_idx].trim().trim_matches('\'').trim_matches('"');
                    let cond = item[arrow_idx + 2..].trim();
                    if context.eval_truthy(cond) {
                        active_classes.push(class_name.to_string());
                    }
                } else {
                    let class_name = item.trim_matches('\'').trim_matches('"');
                    if !class_name.is_empty() {
                        active_classes.push(class_name.to_string());
                    }
                }
            }
            if !active_classes.is_empty() {
                format!("class=\"{}\"", active_classes.join(" "))
            } else {
                String::new()
            }
        }).to_string();

        // @style(['background-color: red' => $hasError])
        s = RE_STYLE.replace_all(&s, |caps: &regex::Captures| {
            let inner = &caps[1];
            let mut active_styles: Vec<String> = Vec::new();
            for item in inner.split(',') {
                let item = item.trim();
                if item.is_empty() {
                    continue;
                }
                if let Some(arrow_idx) = item.find("=>") {
                    let style_rule = item[..arrow_idx].trim().trim_matches('\'').trim_matches('"');
                    let cond = item[arrow_idx + 2..].trim();
                    if context.eval_truthy(cond) {
                        active_styles.push(style_rule.to_string());
                    }
                } else {
                    let style_rule = item.trim_matches('\'').trim_matches('"');
                    if !style_rule.is_empty() {
                        active_styles.push(style_rule.to_string());
                    }
                }
            }
            if !active_styles.is_empty() {
                format!("style=\"{}\"", active_styles.join("; "))
            } else {
                String::new()
            }
        }).to_string();

        s
    }

    /// Penanganan @json, @dump, @dd
    fn process_debug_helpers(&self, content: &str, context: &BladeContext) -> String {
        let mut s = content.to_string();

        // @json($variable)
        s = RE_JSON.replace_all(&s, |caps: &regex::Captures| {
            let var = caps[1].trim();
            if let Some(val) = context.get(var) {
                serde_json::to_string(val).unwrap_or_else(|_| "null".to_string())
            } else {
                "null".to_string()
            }
        }).to_string();

        // @dump($variable) / @dd($variable)
        s = RE_DUMP.replace_all(&s, |caps: &regex::Captures| {
            let var = caps[1].trim();
            let json_str = if let Some(val) = context.get(var) {
                serde_json::to_string_pretty(val).unwrap_or_default()
            } else {
                format!("Variable '{}' not found", var)
            };
            format!(
                "<pre style=\"background:#181825;color:#a6e3a1;padding:12px;border-radius:8px;font-family:monospace;font-size:12px;overflow-x:auto;\">{}</pre>",
                html_escape::encode_safe(&json_str)
            )
        }).to_string();

        s
    }

    /// Penanganan direktif Indonesian Script (@is, @script)
    fn process_indonesian_script(&self, content: &str) -> String {
        RE_IS.replace_all(content, |caps: &regex::Captures| {
            let script_name = caps[1].trim();
            let mut file_path = format!("resources/scripts/{}", script_name);
            if !file_path.ends_with(".is") {
                file_path.push_str(".is");
            }

            let source = if let Ok(content) = fs::read_to_string(&file_path) {
                content
            } else {
                let alt_path = format!("resources/js/{}", script_name);
                let alt_path_is = if alt_path.ends_with(".is") { alt_path.clone() } else { format!("{}.is", alt_path) };
                fs::read_to_string(&alt_path_is).unwrap_or_else(|_| format!("// File script '.is' tidak ditemukan: {}", file_path))
            };

            let compiled_js = indonesian_script::compile_is(&source);
            format!("<script>\n{}\n</script>", compiled_js)
        }).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_blade_extends_and_yield() {
        let mut engine = BladeEngine::empty();
        engine.templates.insert("layouts/app.blade.rs".to_string(), "<html><title>@yield('title', 'Default Title')</title><body>@yield('content')</body></html>".to_string());
        engine.templates.insert("home.blade.rs".to_string(), "@extends('layouts.app')\n@section('title', 'Halaman Utama')\n@section('content')<h1>Halo {{ $name }}</h1>@endsection".to_string());

        let mut ctx = BladeContext::new();
        ctx.insert("name", "Pakem Pure Blade");

        let html = engine.render("home", &ctx);
        assert!(html.contains("<title>Halaman Utama</title>"));
        assert!(html.contains("<h1>Halo Pakem Pure Blade</h1>"));
    }

    #[test]
    fn test_blade_conditionals_nested() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        @if ($isAdmin)
            <h1>Halo Admin</h1>
            @if ($role == 'superadmin')
                <span>Full Access</span>
            @else
                <span>Normal Admin</span>
            @endif
        @else
            <h1>Halo Tamu</h1>
        @endif
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("isAdmin", true);
        ctx.insert("role", "superadmin");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Halo Admin"));
        assert!(html.contains("Full Access"));
        assert!(!html.contains("Halo Tamu"));
    }

    #[test]
    fn test_blade_foreach_and_loop_variable() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        <ul>
        @foreach ($users as $user)
            <li>{{ $loop.iteration }}. {{ $user.name }} (first: {{ $loop.first }})</li>
        @endforeach
        </ul>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("users", json!([
            {"name": "Alice"},
            {"name": "Bob"}
        ]));

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("1. Alice (first: true)"));
        assert!(html.contains("2. Bob (first: false)"));
    }

    #[test]
    fn test_blade_forelse_empty() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        @forelse ($items as $item)
            <p>{{ $item }}</p>
        @empty
            <p class="empty">Tidak ada item</p>
        @endforelse
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("items", json!([]));

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Tidak ada item"));
    }

    #[test]
    fn test_blade_auth_guest() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        @auth
            <p>Welcome back, {{ $email }}</p>
        @else
            <a href="/login">Login</a>
        @endauth
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("user_id", 1);
        ctx.insert("email", "admin@pakem.my.id");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Welcome back, admin@pakem.my.id"));
        assert!(!html.contains("Login"));
    }

    #[test]
    fn test_blade_attribute_helpers() {
        let engine = BladeEngine::empty();

        let tmpl = r#"<input type="checkbox" @checked($isActive)> <button @class(['btn', 'btn-primary' => $isPrimary, 'hidden' => $isHidden])>Kirim</button>"#;

        let mut ctx = BladeContext::new();
        ctx.insert("isActive", true);
        ctx.insert("isPrimary", true);
        ctx.insert("isHidden", false);

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("checked"));
        assert!(html.contains("class=\"btn btn-primary\""));
    }

    #[test]
    fn test_blade_null_coalescing() {
        let engine = BladeEngine::empty();

        let tmpl = r#"<p>{{ $username ?? 'Pengguna Anonim' }}</p>"#;
        let ctx = BladeContext::new();

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Pengguna Anonim"));
    }

    #[test]
    fn test_blade_switch_case() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        @switch ($status)
            @case ('pending')
                <span>Menunggu Konfirmasi</span>
                @break
            @case ('active')
                <span>Akun Aktif</span>
                @break
            @default
                <span>Status Tidak Dikenal</span>
        @endswitch
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("status", "active");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Akun Aktif"));
        assert!(!html.contains("Menunggu Konfirmasi"));
    }

    #[test]
    fn test_blade_for_loop() {
        let engine = BladeEngine::empty();

        let tmpl = r#"@for ($i in 1..4)<span>{{ $i }}</span>@endfor"#;
        let ctx = BladeContext::new();

        let html = engine.render_string(tmpl, &ctx);
        assert_eq!(html, "<span>1</span><span>2</span><span>3</span>");
    }

    #[test]
    fn test_blade_dot_notation() {
        let engine = BladeEngine::empty();

        let tmpl = r#"<p>{{ $user.profile.address.city }}</p>"#;
        let mut ctx = BladeContext::new();
        ctx.insert("user", json!({
            "profile": {
                "address": {
                    "city": "Bandung"
                }
            }
        }));

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Bandung"));
    }

    #[test]
    fn test_blade_native_and_chained_filters() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        <p>Harga: {{ $harga | rupiah }}</p>
        <p>Slug: {{ $judul | slug }}</p>
        <p>Rantai: {{ $judul | slug | uppercase }}</p>
        <p>Potong: {{ $deskripsi | limit: 10 }}</p>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("harga", 150000);
        ctx.insert("judul", "Pakem Web Framework Rust!");
        ctx.insert("deskripsi", "Ini adalah deskripsi yang sangat panjang sekali");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("<p>Harga: Rp 150.000</p>"));
        assert!(html.contains("<p>Slug: pakem-web-framework-rust</p>"));
        assert!(html.contains("<p>Rantai: PAKEM-WEB-FRAMEWORK-RUST</p>"));
        assert!(html.contains("<p>Potong: Ini adalah...</p>"));
    }

    #[test]
    fn test_blade_wasm_filter_pipeline() {
        struct TestPluginRunner;
        impl BladePluginRunner for TestPluginRunner {
            fn call_filter(&self, plugin_name: &str, func_name: &str, arg: &str) -> Result<String, String> {
                if plugin_name == "filter_sensor" && func_name == "sensor" {
                    return Ok(arg.replace("xxxx", "****"));
                }
                if plugin_name == "text_tool" && func_name == "kapital" {
                    return Ok(arg.to_uppercase());
                }
                Ok(arg.to_string())
            }

            fn eval_is_script(&self, script_path: &str, func_name: &str, args: &[Value]) -> Result<Value, String> {
                if script_path == "promo.is" && func_name == "hitung_promo" {
                    if let Some(total) = args.get(0).and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))) {
                        return Ok(serde_json::json!(total * 0.50));
                    }
                }
                Ok(serde_json::json!(0))
            }
        }

        let mut engine = BladeEngine::empty().with_plugin_runner(Arc::new(TestPluginRunner));
        engine.register_wasm_filter("sensor", "filter_sensor", "sensor");

        let tmpl = r#"<p>Komentar: {{ $pesan | sensor }}</p>"#;
        let mut ctx = BladeContext::new();
        ctx.insert("pesan", "teks rahasia xxxx");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("<p>Komentar: teks rahasia ****</p>"));
    }

    #[test]
    fn test_blade_wasm_directive() {
        struct TestPluginRunner;
        impl BladePluginRunner for TestPluginRunner {
            fn call_filter(&self, plugin_name: &str, func_name: &str, arg: &str) -> Result<String, String> {
                if plugin_name == "text_tool" && func_name == "kapital" {
                    return Ok(arg.to_uppercase());
                }
                Ok(arg.to_string())
            }
        }

        let engine = BladeEngine::empty().with_plugin_runner(Arc::new(TestPluginRunner));

        let tmpl = r#"<div>Hasil: @wasm('text_tool', 'kapital', $nama)</div>"#;
        let mut ctx = BladeContext::new();
        ctx.insert("nama", "canting hybrid");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("<div>Hasil: CANTING HYBRID</div>"));
    }

    #[test]
    fn test_blade_eval_is_directive() {
        struct TestPluginRunner;
        impl BladePluginRunner for TestPluginRunner {
            fn eval_is_script(&self, script_path: &str, func_name: &str, args: &[Value]) -> Result<Value, String> {
                if script_path == "promo.is" && func_name == "hitung_promo" {
                    if let Some(total) = args.get(0).and_then(|v| v.as_f64().or_else(|| v.as_i64().map(|i| i as f64))) {
                        return Ok(serde_json::json!(total * 0.50));
                    }
                }
                Ok(serde_json::json!(0))
            }
        }

        let engine = BladeEngine::empty().with_plugin_runner(Arc::new(TestPluginRunner));

        let tmpl = r#"<span>Hemat: Rp @eval_is('promo.is', 'hitung_promo', $total, 'DISKON50')</span>"#;
        let mut ctx = BladeContext::new();
        ctx.insert("total", 100000);

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("<span>Hemat: Rp 50000</span>"));
    }

    #[test]
    fn test_blade_form_validation_and_helpers() {
        let engine = BladeEngine::empty();

        let tmpl = r#"
        <form method="POST" action="/register">
            @csrf
            <input type="email" name="email" value="{{ old('email', 'guest@example.com') }}">
            @error('email')
                <span class="error-msg">{{ $message }}</span>
            @enderror

            <input type="password" name="password">
            @error('password')
                <span class="error-msg">{{ $message }}</span>
            @enderror

            <input type="text" name="phone" value="{{ old('phone', '0812000000') }}">

            <link rel="stylesheet" href="{{ asset('css/custom.css') }}">

            @if(error('email'))
                <div class="alert alert-danger">Ada error pada input email!</div>
            @endif

            @if(anyErrors)
                <div class="summary">Formulir memiliki error validasi.</div>
            @endif
        </form>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("csrf_token", "sec-token-123");

        // Suntikkan errors dan old input persis seperti yang dilakukan ViewEngine::render_with_session
        let errors_json = serde_json::json!({
            "email": "Email ini sudah digunakan oleh akun lain."
        });
        ctx.insert("errors", errors_json);

        let old_json = serde_json::json!({
            "email": "salim@example.com"
        });
        ctx.insert("old", old_json);

        let html = engine.render_string(tmpl, &ctx);

        // 1. Verifikasi old() mengambil nilai lama
        assert!(html.contains("value=\"salim@example.com\""));
        // 2. Verifikasi old() fallback ke default jika field tidak ada di old input
        assert!(html.contains("value=\"0812000000\""));
        // 3. Verifikasi @error('email') merender $message
        assert!(html.contains("<span class=\"error-msg\">Email ini sudah digunakan oleh akun lain.</span>"));
        // 4. Verifikasi @error('password') yang tidak ada error TIDAK dirender
        assert!(!html.contains("@error('password')"));
        assert!(!html.contains("@enderror"));
        // 5. Verifikasi asset() helper
        assert!(html.contains("href=\"/css/custom.css\""));
        // 6. Verifikasi error('email') dan anyErrors di @if
        assert!(html.contains("Ada error pada input email!"));
        assert!(html.contains("Formulir memiliki error validasi."));
        // 7. Verifikasi @csrf terisi
        assert!(html.contains("value=\"sec-token-123\""));
    }

    #[test]
    fn test_blade_asset_concatenation() {
        let engine = BladeEngine::empty();
        let tmpl = r#"
            <img src="{{ asset('img/gege/logogegefun.avif') }}">
            <img src="{{ asset('storage/images/meals/' . $meal.image) }}">
            <img src="{{ asset('storage/images/' . $folder . '/' . $filename) }}">
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("meal", serde_json::json!({ "image": "meals_123.jpg" }));
        ctx.insert("folder", "event");
        ctx.insert("filename", "party.png");

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("src=\"/img/gege/logogegefun.avif\""));
        assert!(html.contains("src=\"/storage/images/meals/meals_123.jpg\""));
        assert!(html.contains("src=\"/storage/images/event/party.png\""));
    }

    #[test]
    fn test_blade_stack_and_push() {
        let engine = BladeEngine::empty();
        let tmpl = r#"
            <head>
                @stack('css')
            </head>
            <body>
                @push('scripts')
                    <script src="/js/app.js"></script>
                @endpush

                @prepend('scripts')
                    <script src="/js/jquery.js"></script>
                @endprepend

                @stack('scripts')
            </body>
        "#;

        let ctx = BladeContext::new();
        let html = engine.render_string(tmpl, &ctx);

        // 1. @stack('css') yang kosong harus bersih (tidak tampil teks @stack('css'))
        assert!(!html.contains("@stack('css')"));
        assert!(!html.contains("@stack"));

        // 2. @prepend harus muncul sebelum @push
        let jquery_pos = html.find("jquery.js").unwrap();
        let app_pos = html.find("app.js").unwrap();
        assert!(jquery_pos < app_pos);

        // 3. Teks direktif @push, @prepend, dan @stack tidak boleh ada di HTML output
        assert!(!html.contains("@push"));
        assert!(!html.contains("@prepend"));
        assert!(!html.contains("@endpush"));
        assert!(!html.contains("@endprepend"));
    }

    #[test]
    fn test_blade_helpers_and_count() {
        let engine = BladeEngine::empty();
        let tmpl = r#"
            @if(isset($funEvent) && count($funEvent) > 0)
                <div class="events">Ada {{ count($funEvent) }} event</div>
            @else
                <div class="no-events">Belum ada event</div>
            @endif

            @if($categories.count() > 0)
                <div class="cats">Ada {{ $categories.count() }} kategori</div>
            @endif

            <a href="{{ route('funDetail', ['slug' => $event.slug]) }}">{{ strtoupper($event.title) }}</a>
            <a href="{{ route('gegeNewsDetail', $art.slug) }}">Baca Berita</a>
            <span>{{ date('d F Y', strtotime($event.date_end)) }}</span>
            <span>{{ tanggalOnly($event.date_start) }}</span>
            <span>Rp {{ uang($order.grand_total) }}</span>
            <span>Jam: {{ substr($event.time_start, 0, 5) }}</span>
            <p>{{ __('Delete Account') }}</p>
            <div>Sisa: {{ number_format(($event.quota ?? 0) - ($event.booked ?? 0)) }}</div>
            <span>First: {{ $items.first()->title }}</span>
            <span>Bracket: {{ $meta['author'] }}</span>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("funEvent", json!([
            {"title": "Konser Musik Tepi Sungai", "slug": "konser-musik"}
        ]));
        ctx.insert("categories", json!([
            {"id": 1, "name": "Event"},
            {"id": 2, "name": "Promo"}
        ]));
        ctx.insert("event", json!({
            "title": "live music akustik",
            "slug": "live-music",
            "date_start": "2024-05-20",
            "date_end": "2024-12-31",
            "time_start": "19:00:00",
            "quota": 100,
            "booked": 15
        }));
        ctx.insert("art", json!({ "slug": "berita-terbaru" }));
        ctx.insert("order", json!({ "grand_total": 150000 }));
        ctx.insert("items", json!([
            {"title": "Item Pertama"}
        ]));
        ctx.insert("meta", json!({ "author": "Pakem Admin" }));

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Ada 1 event"));
        assert!(!html.contains("Belum ada event"));
        assert!(html.contains("Ada 2 kategori"));
        assert!(html.contains("href=\"/funDetail/live-music\""));
        assert!(html.contains("LIVE MUSIC AKUSTIK"));
        assert!(html.contains("href=\"/gegeNewsDetail/berita-terbaru\""));
        assert!(html.contains("31 December 2024"));
        assert!(html.contains("20 Mei 2024"));
        assert!(html.contains("Rp 150.000"));
        assert!(html.contains("Jam: 19:00"));
        assert!(html.contains("<p>Delete Account</p>"));
        assert!(html.contains("Sisa: 85"));
        assert!(html.contains("First: Item Pertama"));
        assert!(html.contains("Bracket: Pakem Admin"));
    }

    #[test]
    fn test_blade_include_with_parameters() {
        let mut engine = BladeEngine::empty();
        engine.templates.insert(
            "partials/badge.blade.rs".to_string(),
            r#"<span class="badge {{ $type }}">{{ $label }} ({{ $count }})</span>"#.to_string(),
        );

        let tmpl = r#"
            <div>
                @include('partials.badge', ['type' => 'success', 'label' => 'Aktif', 'count' => 10])
                @include('partials.badge', ['type' => $badgeType, 'label' => $badgeLabel, 'count' => $badgeCount])
                @includeWhen(true, 'partials.badge', ['type' => 'info', 'label' => 'Pending', 'count' => 1])
                @includeUnless(false, 'partials.badge', ['type' => 'warning', 'label' => 'Warning', 'count' => 0])
            </div>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("badgeType", "danger");
        ctx.insert("badgeLabel", "Nonaktif");
        ctx.insert("badgeCount", 5);

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("class=\"badge success\""));
        assert!(html.contains("Aktif (10)"));
        assert!(html.contains("class=\"badge danger\""));
        assert!(html.contains("Nonaktif (5)"));
        assert!(html.contains("Pending (1)"));
        assert!(html.contains("Warning (0)"));
    }

    #[test]
    fn test_blade_components() {
        let mut engine = BladeEngine::empty();
        engine.templates.insert(
            "components/alert.blade.rs".to_string(),
            r#"<div class="alert alert-{{ $type }}"><h4>{{ $title }}</h4><p>{{ $slot }}</p></div>"#.to_string(),
        );
        engine.templates.insert(
            "components/card.blade.rs".to_string(),
            r#"<div class="card"><div class="card-header">{{ $header }}</div><div class="card-body">{{ $slot }}</div></div>"#.to_string(),
        );

        let tmpl = r#"
            <x-alert type="danger" title="Peringatan!">Terjadi kesalahan sistem.</x-alert>
            <x-card>
                <x-slot:header>Judul Kartu</x-slot>
                Konten isi kartu di sini
            </x-card>
        "#;

        let ctx = BladeContext::new();
        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("class=\"alert alert-danger\""));
        assert!(html.contains("<h4>Peringatan!</h4>"));
        assert!(html.contains("<p>Terjadi kesalahan sistem.</p>"));
        assert!(html.contains("<div class=\"card-header\">Judul Kartu</div>"));
        assert!(html.contains("<div class=\"card-body\">Konten isi kartu di sini</div>"));
    }

    #[test]
    fn test_blade_vite_and_mix() {
        let engine = BladeEngine::empty();
        let tmpl = r#"
            @vite(['resources/css/app.css', 'resources/js/app.js'])
            <script src="{{ mix('js/vendor.js') }}"></script>
        "#;

        let ctx = BladeContext::new();
        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("<link rel=\"stylesheet\" href=\"/css/app.css\">"));
        assert!(html.contains("<script type=\"module\" src=\"/js/app.js\"></script>"));
        assert!(html.contains("src=\"/js/vendor.js\""));
    }

    #[test]
    fn test_blade_production_shield_and_minify() {
        let engine = BladeEngine::empty();
        let tmpl = r#"
            <!DOCTYPE html>
            <html>
                <head><title>Test</title></head>
                <body>
                    <h1>Rahasia Dagang</h1>
                    @productionShield
                </body>
            </html>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("__minify_html", true);

        let html = engine.render_string(tmpl, &ctx);
        // Memastikan CSS dan JS shield disuntikkan
        assert!(html.contains("_pakem_shield_css"));
        assert!(html.contains("_pakem_shield_js"));
        assert!(html.contains("contextmenu"));
        assert!(html.contains("user-select:none!important"));
        assert!(html.contains("debugger"));

        // Memastikan HTML diminifikasi (tidak ada multiple newline kosong berurutan)
        assert!(!html.contains("\n\n"));
    }

    #[test]
    fn test_blade_new_helpers() {
        let engine = BladeEngine::empty();
        let tmpl = r#"
            <p>{{ Str::limit($longText, 10) }}</p>
            <p>{{ Str::slug('Judul Artikel Keren') }}</p>
            <p>Active: {{ request()->is('admin*') ? 'YES' : 'NO' }}</p>
            <p>User: {{ auth()->user()->name }}</p>
            <p>Flash: {{ session('success', 'Tidak ada pesan') }}</p>
        "#;

        let mut ctx = BladeContext::new();
        ctx.insert("longText", "Ini adalah teks yang sangat panjang sekali");
        ctx.insert("request_path", "/admin/users");
        ctx.insert("user", json!({ "id": 1, "name": "Super Admin" }));
        ctx.insert("session", json!({ "success": "Data berhasil disimpan!" }));

        let html = engine.render_string(tmpl, &ctx);
        assert!(html.contains("Ini adalah..."));
        assert!(html.contains("judul-artikel-keren"));
        assert!(html.contains("Active: YES"));
        assert!(html.contains("User: Super Admin"));
        assert!(html.contains("Flash: Data berhasil disimpan!"));
    }
}


