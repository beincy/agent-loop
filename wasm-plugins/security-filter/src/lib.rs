/// 安全过滤插件
///
/// 检查事件 JSON 中是否包含危险命令（rm / sudo）。
/// 若检测到则拒绝（allow: false），否则放行。
///
/// 内存协议（与 agent-loop 约定）：
///   host 调用 alloc(len) 分配输入缓冲区，写入后调用 process(ptr, len)
///   process 返回 [4字节 LE u32 长度][JSON 字节] 的指针
///   host 可选调用 dealloc(ptr, size) 释放
#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let ptr = buf.as_mut_ptr() as i32;
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn dealloc(ptr: i32, size: i32) {
    if ptr == 0 || size == 0 {
        return;
    }
    unsafe {
        let _ = Vec::from_raw_parts(ptr as *mut u8, size as usize, size as usize);
    }
}

#[no_mangle]
pub extern "C" fn process(ptr: i32, len: i32) -> i32 {
    // 读取输入
    let input = unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) };
    let text = std::str::from_utf8(input).unwrap_or("");

    let allow = !contains_dangerous_command(text);

    // 构造输出 JSON
    let json: &[u8] = if allow {
        b"{\"allow\":true}"
    } else {
        b"{\"allow\":false}"
    };

    // 输出格式：[4字节 LE u32 长度][JSON 字节]
    let mut out: Vec<u8> = Vec::with_capacity(4 + json.len());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(json);

    let out_ptr = out.as_ptr() as i32;
    std::mem::forget(out);
    out_ptr
}

// ── 危险命令检测 ───────────────────────────────────────────────

/// 检测整个事件文本中是否出现 rm 或 sudo 命令
fn contains_dangerous_command(text: &str) -> bool {
    contains_as_word(text, "rm") || contains_as_word(text, "sudo")
}

/// 检查 needle 是否以"词边界"形式出现在 haystack 中（大小写不敏感）
/// 词字符定义：ASCII 字母、数字、下划线
fn contains_as_word(haystack: &str, needle: &str) -> bool {
    let bytes = haystack.as_bytes();
    let needle_bytes = needle.as_bytes();
    let nlen = needle_bytes.len();

    if bytes.len() < nlen {
        return false;
    }

    let mut i = 0;
    while i + nlen <= bytes.len() {
        // 大小写不敏感比较
        if bytes[i..i + nlen].eq_ignore_ascii_case(needle_bytes) {
            let before_ok = i == 0 || !is_word_char(bytes[i - 1]);
            let after_ok = i + nlen >= bytes.len() || !is_word_char(bytes[i + nlen]);
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

// ── 单元测试 ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rm() {
        assert!(contains_dangerous_command("please run: rm -rf /tmp/foo"));
        assert!(contains_dangerous_command("cmd: rm file.txt"));
        assert!(contains_dangerous_command("RM -rf /"));         // 大写
        assert!(contains_dangerous_command("{\"cmd\":\"rm\"}"));  // 独立 rm
    }

    #[test]
    fn detects_sudo() {
        assert!(contains_dangerous_command("sudo apt-get install foo"));
        assert!(contains_dangerous_command("SUDO rm -rf /"));
        assert!(contains_dangerous_command("{\"run\":\"sudo make\"}"));
    }

    #[test]
    fn allows_safe_words() {
        // "rm" 作为单词的一部分，不应触发
        assert!(!contains_dangerous_command("terraform apply"));
        assert!(!contains_dangerous_command("form submit"));
        assert!(!contains_dangerous_command("alarm clock"));
        assert!(!contains_dangerous_command("permissions: read"));
        assert!(!contains_dangerous_command("normal webhook payload"));
    }

    #[test]
    fn allows_empty() {
        assert!(!contains_dangerous_command(""));
        assert!(!contains_dangerous_command("{}"));
    }
}
