use super::*;

pub(crate) fn emit_log(path: &Path, offset: u64, label: &str, labeled: bool) -> io::Result<u64> {
    if !path.exists() {
        return Ok(offset);
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = String::new();
    file.read_to_string(&mut buffer)?;
    let new_offset = file.stream_position()?;

    if !buffer.is_empty() {
        if labeled {
            for line in buffer.lines() {
                println!("[{label}] {line}");
            }
        } else {
            print!("{buffer}");
        }
        let _ = std::io::stdout().flush();
    }

    Ok(new_offset)
}

pub(crate) fn read_log_tail(path: &Path, tail_bytes: usize) -> io::Result<Option<Vec<u8>>> {
    if !path.exists() {
        return Ok(None);
    }

    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(tail_bytes as u64);
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(Some(buffer))
}

pub(crate) fn latest_non_empty_line(path: &Path, tail_bytes: usize) -> io::Result<Option<String>> {
    let Some(buffer) = read_log_tail(path, tail_bytes)? else {
        return Ok(None);
    };
    if buffer.is_empty() {
        return Ok(None);
    }

    let text = String::from_utf8_lossy(&buffer);
    let line = text
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.to_string());
    Ok(line)
}

pub(crate) fn latest_log_mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
}

pub fn latest_job_log_line(
    jobs_root: &Path,
    job_id: &str,
    tail_bytes: usize,
) -> io::Result<Option<LatestLogLine>> {
    let paths = paths_for(jobs_root, job_id);
    let stdout_line = latest_non_empty_line(&paths.stdout_path, tail_bytes)?;
    let stderr_line = latest_non_empty_line(&paths.stderr_path, tail_bytes)?;

    match (stdout_line, stderr_line) {
        (Some(line), None) => Ok(Some(LatestLogLine {
            stream: LatestLogStream::Stdout,
            line,
        })),
        (None, Some(line)) => Ok(Some(LatestLogLine {
            stream: LatestLogStream::Stderr,
            line,
        })),
        (Some(stdout), Some(stderr)) => {
            let stdout_mtime = latest_log_mtime(&paths.stdout_path);
            let stderr_mtime = latest_log_mtime(&paths.stderr_path);
            let prefer_stderr = match (stdout_mtime, stderr_mtime) {
                (Some(out), Some(err)) => err >= out,
                (None, Some(_)) => true,
                _ => false,
            };
            if prefer_stderr {
                Ok(Some(LatestLogLine {
                    stream: LatestLogStream::Stderr,
                    line: stderr,
                }))
            } else {
                Ok(Some(LatestLogLine {
                    stream: LatestLogStream::Stdout,
                    line: stdout,
                }))
            }
        }
        (None, None) => Ok(None),
    }
}

pub(crate) fn follow_poll_delay(advanced: bool, idle_polls: &mut u32) -> StdDuration {
    if advanced {
        *idle_polls = 0;
        return StdDuration::from_millis(15);
    }

    *idle_polls = idle_polls.saturating_add(1);
    let millis = match *idle_polls {
        1 => 40,
        2 => 80,
        3 => 160,
        _ => 240,
    };
    StdDuration::from_millis(millis)
}

pub(crate) fn reconcile_running_job_liveness_for_follow(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let _lock = match SchedulerLock::acquire(jobs_root) {
        Ok(lock) => lock,
        Err(err) if err.to_string().contains("scheduler is busy") => return Ok(()),
        Err(err) => return Err(err),
    };

    let record = read_record(jobs_root, job_id)?;
    if record.status != JobStatus::Running {
        return Ok(());
    }

    let _ = reconcile_running_job_liveness_locked(project_root, jobs_root, binary, &[record])?;
    Ok(())
}

pub fn tail_job_logs(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
    stream: LogStream,
    follow: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut stdout_offset = 0u64;
    let mut stderr_offset = 0u64;
    let mut idle_polls = 0u32;

    let label_stdout = matches!(stream, LogStream::Both);
    let label_stderr = matches!(stream, LogStream::Both);

    loop {
        let mut advanced = false;
        if matches!(stream, LogStream::Stdout | LogStream::Both) {
            let next = emit_log(&paths.stdout_path, stdout_offset, "stdout", label_stdout)?;
            if next != stdout_offset {
                advanced = true;
                stdout_offset = next;
            }
        }

        if matches!(stream, LogStream::Stderr | LogStream::Both) {
            let next = emit_log(&paths.stderr_path, stderr_offset, "stderr", label_stderr)?;
            if next != stderr_offset {
                advanced = true;
                stderr_offset = next;
            }
        }

        if !follow {
            break;
        }

        reconcile_running_job_liveness_for_follow(project_root, jobs_root, binary, job_id)?;

        let record = read_record(jobs_root, job_id)?;
        let running = job_is_active(record.status);
        if !running && !advanced {
            break;
        }

        thread::sleep(follow_poll_delay(advanced, &mut idle_polls));
    }

    Ok(())
}

pub(crate) fn read_log_chunk(path: &Path, offset: u64) -> io::Result<(u64, Vec<u8>)> {
    if !path.exists() {
        return Ok((offset, Vec::new()));
    }

    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    let new_offset = file.stream_position()?;
    Ok((new_offset, buffer))
}

pub fn follow_job_logs_raw(
    project_root: &Path,
    jobs_root: &Path,
    binary: &Path,
    job_id: &str,
) -> Result<i32, Box<dyn std::error::Error>> {
    let paths = paths_for(jobs_root, job_id);
    let mut stdout_offset = 0u64;
    let mut stderr_offset = 0u64;
    let mut idle_polls = 0u32;

    loop {
        let mut advanced = false;

        let (next_stdout, stdout_buf) = read_log_chunk(&paths.stdout_path, stdout_offset)?;
        if !stdout_buf.is_empty() {
            io::stdout().write_all(&stdout_buf)?;
            io::stdout().flush()?;
            advanced = true;
        }
        stdout_offset = next_stdout;

        let (next_stderr, stderr_buf) = read_log_chunk(&paths.stderr_path, stderr_offset)?;
        if !stderr_buf.is_empty() {
            io::stderr().write_all(&stderr_buf)?;
            io::stderr().flush()?;
            advanced = true;
        }
        stderr_offset = next_stderr;

        reconcile_running_job_liveness_for_follow(project_root, jobs_root, binary, job_id)?;

        let record = read_record(jobs_root, job_id)?;
        let running = job_is_active(record.status);
        if !running && !advanced {
            return Ok(record.exit_code.unwrap_or(1));
        }

        thread::sleep(follow_poll_delay(advanced, &mut idle_polls));
    }
}
