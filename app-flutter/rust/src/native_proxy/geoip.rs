//! MaxMind DB（`.mmdb`）只读解析器：给 `GEOIP` 规则提供「这个 IP 属于哪个国家」。
//!
//! **格式依据**（2026-11-16 按规范逐条实现，不猜）：
//!   · MaxMind DB File Format Specification v2.0：文件 = `search tree` + `data section` +
//!     `metadata`；元数据以 `\xAB\xCD\xEF MaxMind.com` 结尾标记，且位于文件最后 128 KiB 内；
//!   · search tree：`node_count` 个节点，`record_size ∈ {24,28,32}` 位；每条记录三种含义 ——
//!     `< node_count` = 下一个节点，`== node_count` = 无数据，`> node_count` = 数据段指针
//!     （偏移 = 记录值 − node_count，相对数据段起点）；
//!   · data section：紧凑的 TLV（控制字节高 3 位是类型，低 5 位是长度；长度 ≥ 29 用扩展字节）；
//!     指针类型 `001` 的 `ss` 两位决定 1–4 字节宽度；
//!   · IPv4 地址查 IPv6 库时按 `::ffff:a.b.c.d` 映射（规范「IPv4-mapped」口径）。
//!
//! **本实现的边界（如实记）**：
//!   · 只读 `country` / `registered_country` / `represented_country` 的 `iso_code`
//!     （再退一层 `continent.code`）；其它字段解出来也不对外暴露；
//!   · 不校验数据库签名 / 版本兼容性，只要求 `binary_format_major_version == 2`；
//!   · 整个文件读进内存（GeoLite2-Country 约 6 MB，引擎启动时读一次）；
//!   · **没有内置数据库**：路径见 [`GeoIp::load_default`]，缺文件时如实返回原因，
//!     `GEOIP` 规则不生效（不静默当成「不是该国」）。

use std::net::IpAddr;
use std::path::{Path, PathBuf};

/// 元数据结尾标记（最后 128 KiB 内最后一次出现的位置）。
const METADATA_MARKER: &[u8] = b"\xab\xcd\xefMaxMind.com";
/// 规范给的上限：元数据在文件最后 128 KiB 内。
const METADATA_WINDOW: usize = 128 * 1024;

/// 一个已加载的 MaxMind DB（只读，线程安全）。
#[derive(Clone)]
pub struct GeoIp {
    data: Vec<u8>,
    node_count: u32,
    record_size: u32,
    ip_version: u8,
    /// 搜索树字节数（数据段起点）。
    tree_size: usize,
    /// 数据段范围 `[data_start, data_end)`。
    data_end: usize,
}

impl GeoIp {
    /// 打开数据库文件。
    pub fn open(path: &Path) -> Result<Self, String> {
        let data = std::fs::read(path)
            .map_err(|e| format!("读取 GeoIP 数据库失败（{}）：{e}", path.display()))?;
        Self::from_bytes(data)
    }

    /// 默认位置：`LUNAC_GEOIP_DB` > `<exe 根>\data\GeoLite2-Country.mmdb`。
    ///
    /// 与 v1（mihomo 的 `data\` 目录）同一约定；缺文件时用一句人话说明怎么补。
    pub fn load_default() -> Result<Self, String> {
        let path = default_path();
        if !path.is_file() {
            return Err(format!(
                "没有 GeoIP 数据库（期望 {}）；放入 GeoLite2-Country.mmdb 或用 LUNAC_GEOIP_DB 指定",
                path.display()
            ));
        }
        Self::open(&path)
    }

    /// 从内存解析（测试与「先读进内存」的调用方共用）。
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, String> {
        let marker_at = find_metadata(&data)
            .ok_or_else(|| "不是 MaxMind DB：没有找到元数据标记".to_string())?;
        let meta_start = marker_at + METADATA_MARKER.len();
        let reader = Reader {
            data: &data,
            base: meta_start,
            end: data.len(),
        };
        let (meta, _) = reader
            .decode(0, 0)
            .map_err(|e| format!("MaxMind DB 元数据无法解析：{e}"))?;

        let major = meta
            .get("binary_format_major_version")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if major != 2 {
            return Err(format!("MaxMind DB 版本不是 2（读到 {major}）"));
        }
        let node_count =
            meta.get("node_count")
                .and_then(Value::as_u64)
                .ok_or_else(|| "MaxMind DB 元数据缺 node_count".to_string())? as u32;
        let record_size =
            meta.get("record_size")
                .and_then(Value::as_u64)
                .ok_or_else(|| "MaxMind DB 元数据缺 record_size".to_string())? as u32;
        if !matches!(record_size, 24 | 28 | 32) {
            return Err(format!("MaxMind DB 的 record_size={record_size} 不受支持"));
        }
        let ip_version =
            meta.get("ip_version")
                .and_then(Value::as_u64)
                .ok_or_else(|| "MaxMind DB 元数据缺 ip_version".to_string())? as u8;
        if !matches!(ip_version, 4 | 6) {
            return Err(format!("MaxMind DB 的 ip_version={ip_version} 不受支持"));
        }

        let tree_size = (node_count as usize)
            .checked_mul(record_size as usize)
            .map(|value| value / 4)
            .ok_or_else(|| "MaxMind DB 的搜索树大小溢出".to_string())?;
        if tree_size > marker_at {
            return Err("MaxMind DB 的搜索树超出文件范围".to_string());
        }
        Ok(Self {
            data,
            node_count,
            record_size,
            ip_version,
            tree_size,
            data_end: marker_at,
        })
    }

    /// 查询国家代码（大写两位，如 `CN`）；查不到 / 库里没有该字段 ⇒ `None`。
    pub fn country_code(&self, ip: IpAddr) -> Option<String> {
        let bytes = address_bytes(ip, self.ip_version)?;
        let mut node = 0u32;
        let bits = self.ip_version as usize * 8;
        for depth in 0..bits {
            let bit = (bytes[depth / 8] >> (7 - depth % 8)) & 1;
            let record = self.record(node, bit)?;
            if record == self.node_count {
                return None;
            }
            if record > self.node_count {
                let offset = (record - self.node_count) as usize;
                let at = self.tree_size.checked_add(offset)?;
                if at >= self.data_end {
                    return None;
                }
                let reader = Reader {
                    data: &self.data,
                    base: self.tree_size,
                    end: self.data_end,
                };
                let (value, _) = reader.decode(offset, 0).ok()?;
                return country_code(&value);
            }
            node = record;
        }
        None
    }

    /// 第 `node` 个节点在 `bit`（0 = 左，1 = 右）方向的记录值。
    fn record(&self, node: u32, bit: u8) -> Option<u32> {
        let index = node as usize;
        if index >= self.node_count as usize {
            return None;
        }
        let at = index * (self.record_size as usize / 4);
        match self.record_size {
            24 => {
                let bytes = self.data.get(at..at + 6)?;
                let (left, right) = (&bytes[..3], &bytes[3..]);
                let picked = if bit == 0 { left } else { right };
                Some(((picked[0] as u32) << 16) | ((picked[1] as u32) << 8) | picked[2] as u32)
            }
            28 => {
                let bytes = self.data.get(at..at + 7)?;
                if bit == 0 {
                    Some((((bytes[0] & 0x0F) as u32) << 24) | u24(&bytes[1..4]))
                } else {
                    Some((((bytes[0] & 0xF0) as u32) << 20) | u24(&bytes[4..7]))
                }
            }
            32 => {
                let bytes = self.data.get(at..at + 8)?;
                let picked = if bit == 0 { &bytes[..4] } else { &bytes[4..] };
                Some(u32::from_be_bytes([
                    picked[0], picked[1], picked[2], picked[3],
                ]))
            }
            _ => None,
        }
    }
}

fn u24(bytes: &[u8]) -> u32 {
    ((bytes[0] as u32) << 16) | ((bytes[1] as u32) << 8) | bytes[2] as u32
}

/// 默认数据库路径（`LUNAC_GEOIP_DB` 优先）。
pub fn default_path() -> PathBuf {
    if let Ok(text) = std::env::var("LUNAC_GEOIP_DB") {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("data")
        .join("GeoLite2-Country.mmdb")
}

/// 把 IP 转成搜索树用的字节序列（位数 = `ip_version * 8`）。
///
/// IPv4 查 IPv6 库时按规范映射成 `::ffff:a.b.c.d`；IPv6 库查 IPv4 地址以外的情况一律查不到。
fn address_bytes(ip: IpAddr, ip_version: u8) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    match (ip, ip_version) {
        (IpAddr::V4(v4), 4) => {
            out[..4].copy_from_slice(&v4.octets());
            Some(out)
        }
        (IpAddr::V6(v6), 4) => {
            // 只认 IPv4-mapped；其余地址在 v4 库里没有对应节点。
            let octets = v6.octets();
            if octets[..10] == [0u8; 10] && octets[10] == 0xff && octets[11] == 0xff {
                out[..4].copy_from_slice(&octets[12..]);
                Some(out)
            } else {
                None
            }
        }
        (IpAddr::V4(v4), 6) => {
            out[10] = 0xff;
            out[11] = 0xff;
            out[12..].copy_from_slice(&v4.octets());
            Some(out)
        }
        (IpAddr::V6(v6), 6) => {
            out.copy_from_slice(&v6.octets());
            Some(out)
        }
        _ => None,
    }
}

/// 在文件最后 128 KiB 里找元数据标记的**最后一次**出现。
fn find_metadata(data: &[u8]) -> Option<usize> {
    if data.len() < METADATA_MARKER.len() {
        return None;
    }
    let start = data.len().saturating_sub(METADATA_WINDOW);
    data[start..]
        .windows(METADATA_MARKER.len())
        .rposition(|window| window == METADATA_MARKER)
        .map(|index| start + index)
}

// ── 数据段 TLV 解码（只做 GEOIP 需要的子集） ──────────────────────────

/// 解出来的值（类型不够用时落到 `Other`，不影响国家码读取）。
#[derive(Clone, Debug, PartialEq)]
enum Value {
    Str(String),
    Map(Vec<(String, Value)>),
    Array(Vec<Value>),
    Uint(u128),
    Int(i64),
    Bool(bool),
    Other,
}

impl Value {
    fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Map(pairs) => pairs.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Uint(value) => u64::try_from(*value).ok(),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(text) => Some(text),
            _ => None,
        }
    }
}

/// 国家码：`country.iso_code` > `registered_country.iso_code` >
/// `represented_country.iso_code` > `continent.code`。
fn country_code(value: &Value) -> Option<String> {
    for (outer, inner) in [
        ("country", "iso_code"),
        ("registered_country", "iso_code"),
        ("represented_country", "iso_code"),
        ("continent", "code"),
    ] {
        if let Some(code) = value
            .get(outer)
            .and_then(|item| item.get(inner))
            .and_then(Value::as_str)
        {
            let code = code.trim();
            if !code.is_empty() {
                return Some(code.to_ascii_uppercase());
            }
        }
    }
    None
}

struct Reader<'a> {
    data: &'a [u8],
    /// 数据段起点（指针偏移相对它；元数据段用它自己）。
    base: usize,
    /// 数据段终点。解码走 `data.len()` 的边界；这里保留字段是为了单测里显式
    /// 构造「只给一段字节」的 Reader，读不读它不影响解析。
    #[allow(dead_code)]
    end: usize,
}

impl Reader<'_> {
    /// 解码 `base + offset` 处的值，返回它与「下一个字节」的偏移。
    fn decode(&self, offset: usize, depth: usize) -> Result<(Value, usize), String> {
        if depth > 32 {
            return Err("嵌套层级过深".to_string());
        }
        let at = self.base.checked_add(offset).ok_or("偏移溢出")?;
        let control = *self.data.get(at).ok_or("控制字节越界")?;
        let mut next = offset + 1;
        let tag = control >> 5;
        let raw_size = (control & 0x1F) as usize;

        // 指针：`001ssvvv`，ss 决定宽度，vvv 是高 3 位（4 字节宽时忽略）。
        if tag == 1 {
            let width = ((control >> 3) & 0x03) as usize + 1;
            let raw = self
                .data
                .get(self.base + next..self.base + next + width)
                .ok_or("指针越界")?;
            let mut value = 0u64;
            for byte in raw {
                value = (value << 8) | *byte as u64;
            }
            if width < 4 {
                value |= ((control & 0x07) as u64) << (width * 8);
            }
            next += width;
            let target = value as usize;
            let (decoded, _) = self.decode(target, depth + 1)?;
            return Ok((decoded, next));
        }

        // 扩展类型：控制字节类型位为 0 时，**下一字节**是真实类型 − 7；
        // 长度扩展字节在它之后（与 maxminddb 解码器一致）。
        let kind = if tag == 0 {
            let extra = *self.data.get(self.base + next).ok_or("扩展类型越界")?;
            next += 1;
            extra as u16 + 7
        } else {
            tag as u16
        };
        // 长度扩展：29 / 30 / 31 分别带 1 / 2 / 3 字节。
        let mut size = raw_size;
        if raw_size >= 29 {
            let extra = raw_size - 28;
            let bytes = self
                .data
                .get(self.base + next..self.base + next + extra)
                .ok_or("长度字段越界")?;
            let mut value = 0usize;
            for byte in bytes {
                value = (value << 8) | *byte as usize;
            }
            size = match extra {
                1 => 29 + value,
                2 => 285 + value,
                _ => 65_821 + value,
            };
            next += extra;
        }

        match kind {
            2 => {
                // UTF-8 字符串
                let bytes = self
                    .data
                    .get(self.base + next..self.base + next + size)
                    .ok_or("字符串越界")?;
                next += size;
                Ok((Value::Str(String::from_utf8_lossy(bytes).to_string()), next))
            }
            3 => {
                // double（8 字节）；GEOIP 不需要数值，跳过。
                next += 8;
                Ok((Value::Other, next))
            }
            4 => {
                // 任意字节
                next += size;
                Ok((Value::Other, next))
            }
            5 | 6 | 9 | 10 => {
                // uint16 / uint32 / uint64 / uint128（大端，长度由 size 给）
                let bytes = self
                    .data
                    .get(self.base + next..self.base + next + size)
                    .ok_or("整数越界")?;
                let mut value: u128 = 0;
                for byte in bytes {
                    value = (value << 8) | *byte as u128;
                }
                next += size;
                Ok((Value::Uint(value), next))
            }
            7 => {
                // map：size = 键值对个数
                let mut pairs = Vec::with_capacity(size.min(64));
                for _ in 0..size {
                    let (key, after_key) = self.decode(next, depth + 1)?;
                    let (value, after_value) = self.decode(after_key, depth + 1)?;
                    next = after_value;
                    if let Value::Str(key) = key {
                        pairs.push((key, value));
                    }
                }
                Ok((Value::Map(pairs), next))
            }
            8 => {
                // int32（补码）
                let bytes = self
                    .data
                    .get(self.base + next..self.base + next + size)
                    .ok_or("int32 越界")?;
                let mut value: i64 = if bytes.first().map(|b| b & 0x80 != 0).unwrap_or(false) {
                    -1
                } else {
                    0
                };
                for byte in bytes {
                    value = (value << 8) | *byte as i64;
                }
                next += size;
                Ok((Value::Int(value), next))
            }
            11 => {
                // array：size = 元素个数
                let mut items = Vec::with_capacity(size.min(64));
                for _ in 0..size {
                    let (item, after) = self.decode(next, depth + 1)?;
                    next = after;
                    items.push(item);
                }
                Ok((Value::Array(items), next))
            }
            14 => Ok((Value::Bool(size != 0), next)),
            _ => Ok((Value::Other, next)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── 最小 MMDB 构造（只为了喂给解析器；格式与规范逐字节一致） ──

    fn ctrl(tag: u8, size: usize) -> Vec<u8> {
        // 测试库只用小尺寸；类型 8 以上按规范走「扩展类型」：控制字节类型位为 0，
        // 下一字节是 `真实类型 − 7`。
        assert!(size < 29, "测试构造器只覆盖小尺寸");
        if tag < 8 {
            vec![(tag << 5) | size as u8]
        } else {
            vec![size as u8, tag - 7]
        }
    }

    fn mstr(text: &str) -> Vec<u8> {
        let mut out = ctrl(2, text.len());
        out.extend_from_slice(text.as_bytes());
        out
    }

    fn muint(tag: u8, value: u64, width: usize) -> Vec<u8> {
        let mut out = ctrl(tag, width);
        let bytes = value.to_be_bytes();
        out.extend_from_slice(&bytes[8 - width..]);
        out
    }

    fn mmap(pairs: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut out = ctrl(7, pairs.len());
        for (key, value) in pairs {
            out.extend_from_slice(&mstr(key));
            out.extend_from_slice(value);
        }
        out
    }

    fn marray(items: &[Vec<u8>]) -> Vec<u8> {
        let mut out = ctrl(11, items.len());
        for item in items {
            out.extend_from_slice(item);
        }
        out
    }

    /// 元数据：v2 的全部必填字段。
    fn metadata(node_count: u64, record_size: u64, ip_version: u64, major: u64) -> Vec<u8> {
        mmap(&[
            ("node_count", muint(6, node_count, 4)),
            ("record_size", muint(5, record_size, 2)),
            ("ip_version", muint(5, ip_version, 2)),
            ("database_type", mstr("Test-Country")),
            ("languages", marray(&[mstr("en"), mstr("zh")])),
            ("binary_format_major_version", muint(5, major, 2)),
            ("binary_format_minor_version", muint(5, 0, 2)),
            ("build_epoch", muint(9, 1_700_000_000, 8)),
            ("description", mmap(&[("en", mstr("unit test"))])),
        ])
    }

    /// 造一个「首字节为 1 的地址 → CN」的库。
    ///
    /// `node_count = 1`：节点 0 的左记录 = 1（无数据），右记录 = 1 + 偏移。
    /// 数据段偏移 0 放一个占位值（指针不允许等于 node_count，否则会被当成「无数据」）。
    fn build_db_with_major(ip_version: u64, record_size: u64, major: u64) -> Vec<u8> {
        let node_count = 1u64;
        let tree_size = (node_count * record_size / 4) as usize;
        // 数据段：占位 1 字节 + 真正的 map。
        let mut data_section = vec![0u8];
        data_section.extend_from_slice(&mmap(&[("country", mmap(&[("iso_code", mstr("CN"))]))]));
        let pointer = node_count + 1; // 指向数据段偏移 1

        let mut tree = Vec::new();
        match record_size {
            24 => {
                tree.extend_from_slice(&[0, 0, 1]);
                tree.extend_from_slice(&(pointer as u32).to_be_bytes()[1..]);
            }
            28 => {
                // 7 字节：b0 高半字节 = 右记录高 4 位，低半字节 = 左记录高 4 位。
                tree.push(0);
                tree.extend_from_slice(&[0, 0, 1]);
                tree.extend_from_slice(&(pointer as u32).to_be_bytes()[1..]);
            }
            32 => {
                tree.extend_from_slice(&(node_count as u32).to_be_bytes());
                tree.extend_from_slice(&(pointer as u32).to_be_bytes());
            }
            other => panic!("unsupported record size {other}"),
        }
        assert_eq!(tree.len(), tree_size);

        let mut file = tree;
        file.extend_from_slice(&data_section);
        file.extend_from_slice(METADATA_MARKER);
        file.extend_from_slice(&metadata(node_count, record_size, ip_version, major));
        file
    }

    fn build_db(ip_version: u64, record_size: u64) -> Vec<u8> {
        build_db_with_major(ip_version, record_size, 2)
    }

    #[test]
    fn a_v4_database_answers_the_right_country() {
        let db = GeoIp::from_bytes(build_db(4, 24)).expect("测试库必须可解析");
        // 1xxxxxxx 的第一位是 1 ⇒ 命中右记录。
        assert_eq!(
            db.country_code("128.0.0.1".parse().unwrap()).as_deref(),
            Some("CN")
        );
        // 0xxxxxxx ⇒ 左记录 = 无数据。
        assert_eq!(db.country_code("1.2.3.4".parse().unwrap()), None);
    }

    #[test]
    fn v4_addresses_are_mapped_into_a_v6_database() {
        let db = GeoIp::from_bytes(build_db(6, 24)).expect("测试库必须可解析");
        assert_eq!(
            db.country_code("8000::1".parse().unwrap()).as_deref(),
            Some("CN")
        );
        // IPv4 映射成 ::ffff:a.b.c.d，首位仍是 0 ⇒ 无数据。
        assert_eq!(db.country_code("1.2.3.4".parse().unwrap()), None);
        assert_eq!(db.country_code("::1".parse().unwrap()), None);
    }

    #[test]
    fn record_sizes_28_and_32_read_the_same_tree() {
        for record_size in [24, 28, 32] {
            let db = GeoIp::from_bytes(build_db(4, record_size)).expect("测试库必须可解析");
            assert_eq!(
                db.country_code("200.0.0.1".parse().unwrap()).as_deref(),
                Some("CN"),
                "record_size={record_size}"
            );
            assert_eq!(db.country_code("10.0.0.1".parse().unwrap()), None);
        }
    }

    #[test]
    fn a_file_without_the_metadata_marker_or_with_version_1_is_rejected() {
        let reason = GeoIp::from_bytes(vec![0u8; 64])
            .err()
            .expect("缺元数据必须失败");
        assert!(reason.contains("元数据标记"), "{reason}");

        let reason = GeoIp::from_bytes(build_db_with_major(4, 24, 1))
            .err()
            .expect("版本 1 必须失败");
        assert!(reason.contains("版本"), "{reason}");
    }

    #[test]
    fn a_geoip_rule_routes_a_literal_ip_by_country() {
        use crate::native_proxy::rules;
        let db = GeoIp::from_bytes(build_db(4, 24)).expect("测试库必须可解析");
        let ip: IpAddr = "128.0.0.1".parse().unwrap();
        let other: IpAddr = "1.2.3.4".parse().unwrap();

        let proxy = rules::parse_rule("GEOIP,CN").expect("GEOIP 规则必须可解析");
        assert!(rules::decide_with_geoip(
            &[proxy],
            "128.0.0.1",
            Some(ip),
            None,
            Some(&db)
        ));

        let direct = rules::parse_rule("GEOIP,CN,DIRECT").expect("带策略位");
        assert!(!rules::decide_with_geoip(
            &[direct.clone()],
            "128.0.0.1",
            Some(ip),
            None,
            Some(&db)
        ));

        // 不是 CN ⇒ 不命中（落到后面的规则 / 兜底）。
        let jp = rules::parse_rule("GEOIP,JP").expect("GEOIP 规则必须可解析");
        assert!(!rules::decide_with_geoip(
            &[jp],
            "128.0.0.1",
            Some(ip),
            None,
            Some(&db)
        ));
        // 库里「无数据」的地址同样不命中。
        assert!(!rules::decide_with_geoip(
            &[direct],
            "1.2.3.4",
            Some(other),
            None,
            Some(&db)
        ));
    }

    #[test]
    fn a_missing_database_reports_the_expected_path() {
        let previous = std::env::var("LUNAC_GEOIP_DB").ok();
        std::env::set_var("LUNAC_GEOIP_DB", "Z:\\definitely\\missing\\geo.mmdb");
        let reason = GeoIp::load_default().err().expect("缺文件必须失败");
        assert!(reason.contains("GeoLite2-Country.mmdb"), "{reason}");
        match previous {
            Some(value) => std::env::set_var("LUNAC_GEOIP_DB", value),
            None => std::env::remove_var("LUNAC_GEOIP_DB"),
        }
    }

    #[test]
    fn country_lookup_falls_back_through_the_documented_paths() {
        let registered = mmap(&[("registered_country", mmap(&[("iso_code", mstr("jp"))]))]);
        let db_value = Reader {
            data: &registered,
            base: 0,
            end: registered.len(),
        }
        .decode(0, 0)
        .expect("map 必须可解码")
        .0;
        assert_eq!(country_code(&db_value).as_deref(), Some("JP"));

        let continent = mmap(&[("continent", mmap(&[("code", mstr("EU"))]))]);
        let value = Reader {
            data: &continent,
            base: 0,
            end: continent.len(),
        }
        .decode(0, 0)
        .expect("map 必须可解码")
        .0;
        assert_eq!(country_code(&value).as_deref(), Some("EU"));
    }
}
