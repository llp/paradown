// =================================================================================
// 1. 模块导入与包引用 (use Statements)
// =================================================================================
// `crate::` 表示从当前项目（Crate）根目录开始的绝对路径，这里导入自定义错误类型 Error
use crate::error::Error;
// `chrono::{DateTime, Utc}`：花括号语法用于在同一模块路径下嵌套导入多个项（Nested Imports）
// DateTime 是日期时间类型，Utc 是 UTC 时区类型
use chrono::{DateTime, Utc};
// `digest::Digest`：导入第三方 trait（特质/接口）。在 Rust 中，调用某个类型在 trait 中定义的方法前，必须导入该 trait
use digest::Digest;
// `log::debug`：导入日志记录宏 debug!
use log::debug;
// 导入各种哈希计算结构体
use md5::Md5;
// `serde::{Deserialize, Serialize}`：导入 serde 库用于序列化（转 JSON 等）与反序列化的 trait
use serde::{Deserialize, Serialize};
use sha1::Sha1;
use sha2::Sha256;
// 标准库 (std) 读写与路径处理相关项
use std::fs::File;
use std::io::{BufReader, Read}; // BufReader 为带缓冲区的读取器，Read 为 I/O 读取 trait
use std::path::Path;            // Path 对应文件路径的切片类型（只读引用，无所有权）
use std::str::FromStr;          // FromStr trait：定义从字符串解析成指定类型的标准接口

// =================================================================================
// 2. 枚举类型 (Enum) 与 派生宏 (Derive Macros)
// =================================================================================

/// `#[derive(...)]` 这是一个属性宏（Attribute Macro），自动为类型实现指定的 Trait：
/// - `Debug`: 允许使用 `{:?}` 占位符打印调试日志
/// - `Clone`: 允许显式深拷贝对象（调用 `.clone()`）
/// - `Serialize`, `Deserialize`: 自动生成 JSON/数据包序列化和反序列化的实现代码
#[derive(Debug, Clone, Serialize, Deserialize)]
// `pub enum`: 声明公有枚举类型。Rust 的 Enum 属于代数数据类型（Sum Type），每个变体可独立存在或包含数据
pub enum ChecksumAlgorithm {
    MD5,
    SHA1,
    SHA256,
    NONE,
}

// =================================================================================
// 3. Trait 实现 (impl Trait for Type)
// =================================================================================

// `impl FromStr for ChecksumAlgorithm`：为 `ChecksumAlgorithm` 实现标准库的 `FromStr` 特质。
// 实现后即可使用 `"MD5".parse::<ChecksumAlgorithm>()` 或 `ChecksumAlgorithm::from_str("MD5")`
impl FromStr for ChecksumAlgorithm {
    // 关联类型 (Associated Type)：Trait 内部定义的类型占位符。
    // 此处设为 `()`（空元组 / unit 类型），表示字符串解析失败时不产生具体的错误信息
    type Err = ();

    // 实现 Trait 要求的函数：
    // - `s: &str`: 输入参数为不可变字符串切片引用（借用，不转移所有权）
    // - `Result<Self, Self::Err>`: 返回标准 Result 枚举，`Self` 指代当前类型（ChecksumAlgorithm）
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // `match` 模式匹配（Pattern Matching），Rust 要求 match 必须穷尽所有可能性（Exhaustive Matching）
        match s {
            "MD5" => Ok(ChecksumAlgorithm::MD5),
            "SHA1" => Ok(ChecksumAlgorithm::SHA1),
            "SHA256" => Ok(ChecksumAlgorithm::SHA256),
            "NONE" => Ok(ChecksumAlgorithm::NONE),
            // `_` 为通配符（Wildcard），匹配任意未显式列出的字符串模式
            _ => Ok(ChecksumAlgorithm::NONE),
        }
    }
}

//--------------------------------------------------------------------------------------------------
// =================================================================================
// 4. 结构体定义 (Struct) 与 Option 类型
// =================================================================================

/// 校验和数据结构体
#[derive(Clone, Debug, Serialize, Deserialize)]
// `pub struct`: 声明公有结构体（Product Type）
pub struct Checksum {
    // `pub`: 结构体字段若需被外部模块访问，必须明确标记为 pub
    pub algorithm: ChecksumAlgorithm,

    // `Option<T>` 为 Rust 处理可能为空（null）的值的标准枚举：
    // - `Some(T)` 表示包含值
    // - `None` 表示无值（从语言层面消除了空指针异常 / NullPointerException）
    pub value: Option<String>,
    pub verified: Option<bool>,
    pub verified_at: Option<DateTime<Utc>>,
}

// =================================================================================
// 5. 结构体方法实现 (impl Struct)
// =================================================================================

impl Checksum {
    // `pub fn verify`: 结构体的实例方法
    // - `&self`: 借用当前结构体实例的不可变引用（不传递所有权，防销毁）
    // - `file_path: &Path`: 路径切片引用
    // - `Result<bool, Error>`: 成功时返回 bool 校验结果，失败时返回 Error
    pub fn verify(&self, file_path: &Path) -> Result<bool, Error> {
        // 对 Option 切片引用匹配解包：`match &self.value`
        let expected = match &self.value {
            // `Some(v)`：如果有值，解包出 `v`（类型为 &String 引用）
            Some(v) => v,
            // `None`：如果为空，打印日志并提前返回 `Ok(true)` 跳过校验
            None => {
                debug!(
                    "[Checksum] No expected value for file: {:?}, skipping verification",
                    file_path
                );
                return Ok(true); // 没有期望值则跳过
            }
        };

        debug!(
            "[Checksum] Verifying file: {:?} using algorithm: {:?}",
            file_path, self.algorithm
        );

        // 匹配枚举 `self.algorithm`
        let actual = match self.algorithm {
            // `?` 问号运算符（错误传播）：
            // 如果内部表达式返回 `Ok(val)`，则自动解包得到 `val`；
            // 如果返回 `Err(err)`，则中断当前函数执行并立即将 `Err` 返回给上层调用方
            ChecksumAlgorithm::MD5 => calculate_md5(file_path)?,
            ChecksumAlgorithm::SHA1 => calculate_sha1(file_path)?,
            ChecksumAlgorithm::SHA256 => calculate_sha256(file_path)?,
            ChecksumAlgorithm::NONE => {
                debug!(
                    "[Checksum] Algorithm NONE for file: {:?}, skipping verification",
                    file_path
                );
                return Ok(true);
            }
        };

        // 比较计算出的实际哈希字符串与预期字符串
        // `&actual` 类型为 `&String`，与 `expected` (`&String`) 校验相等性
        let result = &actual == expected;
        debug!(
            "[Checksum] File: {:?}, Algorithm: {:?}, Expected: {}, Actual: {}, Result: {}",
            file_path, self.algorithm, expected, actual, result
        );

        // Rust 中函数最后一行如果不加分号 `;`，该表达式的结果即为函数的返回值
        Ok(result)
    }
}

// =================================================================================
// 6. 私有辅助函数 (Private Helper Functions) 与 I/O 缓冲区读写
// =================================================================================

// 没有 `pub` 前缀的函数属于私有函数，仅在当前模块/文件内部可见
fn calculate_md5(file_path: &Path) -> Result<String, Error> {
    debug!("[Checksum] Calculating MD5 for file: {:?}", file_path);

    // =================================================================================
    // 【语法深度详解】 `let file = File::open(file_path).map_err(|e| Error::Other(e.to_string()))?;`
    // ---------------------------------------------------------------------------------
    // 1. `File::open(file_path)`:
    //    调用 std::fs::File 的静态方法打开文件，返回 `Result<File, std::io::Error>`。
    //    如果成功为 `Ok(File)`，如果失败（如文件不存在）为 `Err(std::io::Error)`。
    //
    // 2. `.map_err(|e| ...)`:
    //    `Result<T, E>` 类型的内置方法。作用是：如果为 Ok 则保持不变；如果为 Err(e)，则调用传入的闭包函数对错误类型 E 进行转换。
    //    `|e| Error::Other(e.to_string())` 是一个匿名闭包（Closure）：
    //    - `|e|` 声明输入参数 `e`（即 std::io::Error）
    //    - `e.to_string()` 将 std::io::Error 转为 String
    //    - `Error::Other(...)` 包装成当前 crate 自定义的 `Error::Other` 枚举变体。
    //    经过 map_err 后，Result 类型从 `Result<File, std::io::Error>` 变成了 `Result<File, crate::error::Error>`。
    //
    // 3. `?` (问号运算符 / 错误传播运算符 - Error Propagation Operator):
    //    `?` 是 Rust 用于简化错误处理的核心语法糖。对于 `Result<T, E>`，它在底层的语义等价于如下模式匹配的展开：
    //    ---------------------------------------------------------------------------------
    //    let file = match File::open(file_path).map_err(|e| Error::Other(e.to_string())) {
    //        Ok(val) => val,                           // 情况 A (成功): 自动剥离/解包 Ok 外壳，提取内部的 File 对象，继续向下执行
    //        Err(err) => return Err(From::from(err)), // 情况 B (失败): 自动执行提前 return，将 Err 抛给上层调用函数
    //    };
    //    ---------------------------------------------------------------------------------
    //    【`?` 运算符四大核心特性】：
    //    ① 自动解包 (Unwrap)：成功时提取内层数据 (`Ok(T)` -> `T`，`Some(T)` -> `T`)。
    //    ② 早期返回 (Early Return)：失败时自动从当前函数 return 退出 (`Err(E)` 或 `None`)。
    //    ③ 错误类型自动转换 (Trait From)：若 `Err` 的类型 `E1` 与当前函数返回错误的类型 `E2` 不同，
    //       只要 `E2` 实现了 `From<E1>`，`?` 会自动调用 `From::from(err)` 进行隐式类型转换。
    //    ④ 作用域使用限制：只能在返回值类型为 `Result` 或 `Option`（或实现了 `Try` Trait）的函数内部使用。
    // =================================================================================
    let file = File::open(file_path).map_err(|e| Error::Other(e.to_string()))?;

    // `let mut`: `mut` 关键字标记变量可变（Mutable）。Rust 变量默认只读不可变，修改状态必须加 mut
    // `BufReader::new(file)`：用缓冲读取器包裹文件，提供默认 8KB 缓存，减少频繁发起磁盘 I/O 系统调用的开销
    let mut reader = BufReader::new(file);

    // 实例化 Md5 哈希器，由于更新哈希状态需要修改内部数据，故声明为 `mut`
    let mut hasher = Md5::new();

    // =================================================================================
    // 【语法深度详解】 `let mut buffer = [0u8; 8192];`
    // ---------------------------------------------------------------------------------
    // 1. `[值; 长度]` (数组重复初始化语法):
    //    这是 Rust 初始化固定长度栈数组的专用语法，表示创建一个长度为 8192 的数组，
    //    并将其中所有 8192 个元素全部填充初始化为前边的初始值 `0u8`。
    //
    // 2. `0u8` (带类型后缀的数值字面量):
    //    - `0`: **正是默认初始值 0**！
    //    - `u8`: 类型后缀，代表无符号 8 位整数（Unsigned 8-bit Integer，即单字节 byte）。
    //    - 整体 `0u8` 表示：类型为 u8、数值为 0 的字节字面量（类似 C 语言的 (uint8_t)0）。
    //
    // 3. 为什么 Rust 必须明确指定初始值 0？
    //    C/C++ 中在栈上声明 `char buf[8192];` 不初始化会保留未知的垃圾野数据（可能导致安全漏洞）；
    //    Rust 拥有极严格的内存安全保证，**禁止读取未初始化的内存**，因此数组创建时必须显式填充初始值。
    // =================================================================================
    let mut buffer = [0u8; 8192];

    // `loop`: Rust 原生无限循环关键字
    loop {
        // `reader.read(&mut buffer)`：传入缓冲区的可变引用 `&mut buffer` 供函数填充写入数据
        // 返回读取到的字节数 `usize`
        let n = reader
            .read(&mut buffer)
            .map_err(|e| Error::Other(e.to_string()))?;

        // 当读取字节数为 0 时，代表已读到文件末尾（EOF）
        if n == 0 {
            break; // 跳出 loop 循环
        }

        // `&buffer[..n]`：切片语法（Slice Syntax）。
        // 截取 `buffer` 从索引 0 到 `n` 的不可变字节切片 `&[u8]`，并送入哈希器更新
        hasher.update(&buffer[..n]);
    }

    // `hasher.finalize()`：完成计算并消耗 hasher，返回 Hash 标量值
    // `format!("{:x}", ...)`：格式化宏，`{:x}` 表示以十六进制小写形式转为字符串
    let hash = format!("{:x}", hasher.finalize());
    debug!("[Checksum] MD5 result for file {:?}: {}", file_path, hash);
    Ok(hash)
}

fn calculate_sha1(file_path: &Path) -> Result<String, Error> {
    debug!("[Checksum] Calculating SHA1 for file: {:?}", file_path);
    let file = File::open(file_path).map_err(|e| Error::Other(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha1::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| Error::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    debug!("[Checksum] SHA1 result for file {:?}: {}", file_path, hash);
    Ok(hash)
}

fn calculate_sha256(file_path: &Path) -> Result<String, Error> {
    debug!("[Checksum] Calculating SHA256 for file: {:?}", file_path);
    let file = File::open(file_path).map_err(|e| Error::Other(e.to_string()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 8192];

    loop {
        let n = reader
            .read(&mut buffer)
            .map_err(|e| Error::Other(e.to_string()))?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }

    let hash = format!("{:x}", hasher.finalize());
    debug!(
        "[Checksum] SHA256 result for file {:?}: {}",
        file_path, hash
    );
    Ok(hash)
}

