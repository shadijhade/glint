//! Encrypt and decrypt with age (the `age` crate, MIT/Apache-2.0, in Glint itself: the format of age and
//! rage, readable with either on any system): a file becomes `name.age`, a folder `name.tar.zst.age`
//! (packed with Zstandard first), locked with a passphrase (scrypt) or to people's age keys (`age1…`).
//! Decrypting gives back the file, or the `.tar.zst` that Glint opens like a folder. Each runs through
//! actions.rs, so what's made lands beside the originals (or in their place) and Undo takes it back.

use crate::actions::{self, Job, Output, Progress, Report};
use crate::fsx::blocking;
use age::secrecy::SecretString;
use serde::Deserialize;
use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::ipc::Channel;
use tauri::Window;

static STOP: AtomicBool = AtomicBool::new(false);

#[tauri::command]
pub fn vault_cancel() {
    STOP.store(true, Ordering::SeqCst);
}

#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum VaultTask {
    /// Locked with `password`, or (when there are any) to these age keys instead.
    Encrypt { password: String, recipients: Vec<String> },
    /// Unlocked with `password`.
    Decrypt { password: String },
}

#[tauri::command]
pub async fn vault_run(window: Window, files: Vec<String>, task: VaultTask, output: Output, on_progress: Channel<Progress>) -> Result<Report, String> {
    let owner = crate::shell::owner(&window);
    STOP.store(false, Ordering::SeqCst);
    blocking(move || {
        if let VaultTask::Encrypt { password, recipients } = &task {
            if password.is_empty() && recipients.is_empty() {
                return Err("Give a passphrase, or someone's age key".into());
            }
            for r in recipients {
                age::x25519::Recipient::from_str(r.trim()).map_err(|_| format!("“{r}” isn't an age key (they start with age1)"))?;
            }
        }
        let jobs: Vec<Job> = files.iter().map(Job::one).collect();
        actions::run(owner, "vault", &jobs, &output, 2, &STOP, &|p| drop(on_progress.send(p)), |job, part, work| {
            let item = &job.inputs[0];
            match &task {
                VaultTask::Encrypt { password, recipients } => encrypt(item, password, recipients, part, work),
                VaultTask::Decrypt { password } => decrypt(item, password, part, work),
            }
            .map(|f| vec![f])
        })
    })
    .await
}

/// Copies `from` into `to`, a block at a time, saying how far it is and stopping when asked.
fn pump(mut from: impl Read, mut to: impl Write, total: u64, part: &(dyn Fn(f32) + Sync)) -> Result<(), String> {
    let mut buffer = vec![0u8; 1 << 20];
    let mut done = 0u64;
    loop {
        if STOP.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let n = from.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        to.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        if total > 0 {
            part(done as f32 / total as f32);
        }
    }
}

fn encrypt(item: &Path, password: &str, recipients: &[String], part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let name = item.file_name().ok_or("It has no name")?.to_string_lossy().into_owned();
    // A folder is packed first (tar, Zstandard), into the work folder.
    let (source, packed) = if item.is_dir() {
        let tar = work.join(format!("{name}.tar.zst"));
        crate::archive::pack_tar_zst(item, &tar, &STOP)?;
        (tar.clone(), Some(tar))
    } else {
        (item.to_path_buf(), None)
    };
    let out = work.join(format!("{}.age", source.file_name().unwrap_or_default().to_string_lossy()));
    let encryptor = if recipients.is_empty() {
        age::Encryptor::with_user_passphrase(SecretString::from(password.to_string()))
    } else {
        let keys: Vec<age::x25519::Recipient> = recipients.iter().filter_map(|r| age::x25519::Recipient::from_str(r.trim()).ok()).collect();
        age::Encryptor::with_recipients(keys.iter().map(|k| k as &dyn age::Recipient)).map_err(|e| e.to_string())?
    };
    let total = std::fs::metadata(&source).map_or(0, |m| m.len());
    let input = BufReader::new(File::open(&source).map_err(|e| e.to_string())?);
    let mut writer = encryptor.wrap_output(BufWriter::new(File::create(&out).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    pump(input, &mut writer, total, part)?;
    writer.finish().and_then(|mut w| w.flush()).map_err(|e| e.to_string())?;
    if let Some(tar) = packed {
        let _ = std::fs::remove_file(tar);
    }
    Ok(out)
}

fn decrypt(file: &Path, password: &str, part: &(dyn Fn(f32) + Sync), work: &Path) -> Result<PathBuf, String> {
    let name = file.file_name().ok_or("It has no name")?.to_string_lossy().into_owned();
    let Some(inner) = name.strip_suffix(".age").or_else(|| name.strip_suffix(".AGE")).filter(|n| !n.is_empty()) else {
        return Err("It isn't an .age file".into());
    };
    let decryptor = age::Decryptor::new_buffered(BufReader::new(File::open(file).map_err(|e| e.to_string())?)).map_err(|_| "It isn't an age file, or it's damaged".to_string())?;
    if !decryptor.is_scrypt() {
        return Err("It's locked to someone's key, not a passphrase: decrypt it with age or rage and the key".into());
    }
    let identity = age::scrypt::Identity::new(SecretString::from(password.to_string()));
    let reader = decryptor.decrypt(std::iter::once(&identity as &dyn age::Identity)).map_err(|e| match e {
        age::DecryptError::DecryptionFailed | age::DecryptError::KeyDecryptionFailed => "The passphrase isn't right".to_string(),
        other => other.to_string(),
    })?;
    let out = work.join(inner);
    let total = std::fs::metadata(file).map_or(0, |m| m.len());
    let mut target = BufWriter::new(File::create(&out).map_err(|e| e.to_string())?);
    pump(reader, &mut target, total, part)?;
    target.flush().map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypts_and_decrypts_with_a_passphrase() {
        let dir = std::env::temp_dir().join(format!("glint-vault-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("work")).unwrap();
        std::fs::create_dir_all(dir.join("back")).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "the secret plan").unwrap();
        let locked = encrypt(&file, "correct horse", &[], &|_| {}, &dir.join("work")).unwrap();
        assert!(locked.ends_with("notes.txt.age"));
        assert!(!std::fs::read(&locked).unwrap().windows(6).any(|w| w == b"secret"));
        assert_eq!(decrypt(&locked, "wrong", &|_| {}, &dir.join("back")).err().unwrap(), "The passphrase isn't right");
        let back = decrypt(&locked, "correct horse", &|_| {}, &dir.join("back")).unwrap();
        assert_eq!(std::fs::read_to_string(back).unwrap(), "the secret plan");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
