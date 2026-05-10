use anyhow::Result;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct ExtractedDocs {
    pub text: String,
    pub method: String,
}

pub fn extract_docs(
    command_name: &str,
    command_path: &str,
    max_doc_bytes: usize,
) -> Result<String> {
    Ok(extract_docs_with_method(command_name, command_path, max_doc_bytes)?.text)
}

pub fn extract_docs_with_method(
    command_name: &str,
    command_path: &str,
    max_doc_bytes: usize,
) -> Result<ExtractedDocs> {
    let candidates = vec![
        ("man", vec!["-P", "cat", "--", command_name], "man"),
        (command_path, vec!["--help"], "--help"),
        (command_path, vec!["-h"], "-h"),
    ];

    for (program, args, method) in candidates {
        if let Some(text) = run_with_timeout(program, &args, Duration::from_secs(2), max_doc_bytes)?
        {
            return Ok(ExtractedDocs {
                text,
                method: method.to_string(),
            });
        }
    }

    Ok(ExtractedDocs {
        text: "no documentation available".to_string(),
        method: "stub".to_string(),
    })
}

fn run_with_timeout(
    program: &str,
    args: &[&str],
    timeout: Duration,
    max_doc_bytes: usize,
) -> Result<Option<String>> {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .env("LANG", "C")
        .env("TERM", "dumb")
        .env("MANPAGER", "cat")
        .env("MANWIDTH", "120")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // SAFETY: runs in child just before exec; sets child PGID to itself so timeout can kill group.
    unsafe {
        cmd.pre_exec(|| {
            let rc = libc::setpgid(0, 0);
            if rc == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return Ok(None),
    };
    let pid = child.id() as i32;
    let start = Instant::now();

    loop {
        if child.try_wait()?.is_some() {
            let out = child.wait_with_output()?;
            let mut merged = out.stdout;
            if !out.stderr.is_empty() {
                if !merged.is_empty() {
                    merged.extend_from_slice(b"\n");
                }
                merged.extend_from_slice(&out.stderr);
            }
            if merged.is_empty() {
                return Ok(None);
            }
            merged.truncate(max_doc_bytes);
            return Ok(Some(String::from_utf8_lossy(&merged).to_string()));
        }

        if start.elapsed() >= timeout {
            // kill the whole process group.
            // SAFETY: kill with negative pgid targets process group created above.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
            let _ = child.wait();
            return Ok(None);
        }

        std::thread::sleep(Duration::from_millis(10));
    }
}
