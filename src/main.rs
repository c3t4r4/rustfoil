#[cfg_attr(test, macro_use)]
extern crate structopt;

use crate::logging::LogLevel::{Debug, Info, Trace};
use anyhow::Error;
use compression::CompressionFlag;
use encryption::EncryptionFlag;
use error::RustfoilError;
use gdrive::GDriveService;
use hhmmss::Hhmmss;
use index::FileEntry;
use index::Index;
use index::ParsedFileInfo;
use indicatif::{ProgressBar, ProgressStyle};
use logging::Logger;
use regex::Regex;
use std::borrow::Borrow;
use std::fs;
use std::path::PathBuf;
use std::time::Instant;
use structopt::StructOpt;
use tinfoil::convert_to_tinfoil_format;

mod compression;
mod encryption;
mod error;
mod gdrive;
mod index;
mod logging;
mod result;
mod tinfoil;

/// Script that will allow you to generate an index file with Google Drive file links for use with Tinfoil
#[derive(StructOpt, Debug)]
#[structopt(rename_all = "kebab-case")]
pub struct Input {
    /// Folder IDs of Google Drive folders to scan
    folder_ids: Vec<String>,

    /// Path to Google Application Credentials
    #[structopt(long, parse(from_os_str), default_value = "credentials.json")]
    credentials: PathBuf,

    /// Path to Google OAuth2.0 User Token
    #[structopt(long, parse(from_os_str), default_value = "token.json")]
    token: PathBuf,

    /// Path to output index file
    #[structopt(short = "o", long, parse(from_os_str), default_value = "index.tfl")]
    output_path: PathBuf,

    /// Share all files inside the index file
    #[structopt(long)]
    share_files: bool,

    /// Scans for files only in top directory for each Folder ID entered
    #[structopt(long)]
    no_recursion: bool,

    /// Adds files without valid Title ID
    #[structopt(long)]
    add_nsw_files_without_title_id: bool,

    /// Adds files without valid NSW ROM extension(NSP/NSZ/XCI/XCZ) to index
    #[structopt(long)]
    add_non_nsw_files: bool,

    /// Adds a success message to index file to show if index is successfully read by Tinfoil
    #[structopt(long)]
    success: Option<String>,

    /// Adds a referrer to index file to prevent others from hotlinking
    #[structopt(long)]
    referrer: Option<String>,

    /// Adds a google API key to be used with all gdrive:/ requests
    #[structopt(long)]
    google_api_key: Option<String>,

    /// Adds 1Fincher API keys to be used with all 1f:/ requests, If multiple keys are provided, Tinfoil keeps trying them until it finds one that works
    #[structopt(long)]
    one_fichier_keys: Option<Vec<String>>,

    /// Adds custom HTTP headers Tinfoil should send with its requests
    #[structopt(long)]
    headers: Option<Vec<String>>,

    /// Adds a minimum Tinfoil version to load the index
    #[structopt(long)]
    min_version: Option<f64>,

    /// Adds a list of themes to blacklist based on their hash
    #[structopt(long)]
    theme_blacklist: Option<Vec<String>>,

    /// Adds a list of themes to whitelist based on their hash
    #[structopt(long)]
    theme_whitelist: Option<Vec<String>>,

    /// Adds a custom theme error message to the index
    #[structopt(long)]
    theme_error: Option<String>,

    /// Path to RSA Public Key to encrypt AES-ECB-256 key with
    #[structopt(long)]
    public_key: Option<PathBuf>,

    /// Shares the index file that is uploaded to Google Drive
    #[structopt(long)]
    share_index: bool,

    /// If the index file should be uploaded to specific folder
    #[structopt(long)]
    upload_folder_id: Option<String>,

    /// If the index file should be uploaded to My Drive
    #[structopt(long)]
    upload_my_drive: bool,

    /// Which compression should be used for the index file
    #[structopt(long, default_value = "zstd")]
    compression: CompressionFlag,

    /// If OAuth should be done headless
    #[structopt(long)]
    headless: bool,

    /// Verbose mode (-v, -vv, -vvv, etc.)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: u8,
}

pub struct RustfoilService {
    logger: Logger,
    input: Input,
    gdrive: Option<GDriveService>,
    timer: Instant,
}

impl RustfoilService {
    pub fn new(input: Input) -> RustfoilService {
        RustfoilService {
            logger: Logger::new(match input.verbose {
                1 => Debug,
                2 => Trace,
                _ => Info,
            }),
            timer: Instant::now(),
            gdrive: None,
            input,
        }
    }

    pub fn init(&mut self) {
        self.gdrive = Some(GDriveService::new(
            self.input.credentials.as_path(),
            self.input.token.as_path(),
            self.input.headless,
        ));
    }

    pub fn validate_input(&self) -> result::Result<()> {
        if !&self.input.credentials.exists() {
            return Err(Error::new(RustfoilError::CredentialsMissing));
        }

        Ok(())
    }

    pub fn generate_index(&self, files: Vec<ParsedFileInfo>) -> result::Result<Box<Index>> {
        let mut index = Box::new(Index::new());

        let mut index_files: Vec<FileEntry> = Vec::new();

        for info in files {
            index_files.push(FileEntry::new(
                format!("gdrive:{}#{}", info.id, info.name_encoded),
                u64::from_str_radix(&*info.size, 10)?,
            ));
        }

        index.files = Some(index_files);

        self.logger.log_debug("Added files to index")?;

        if let Some(success) = &self.input.success {
            index.success = Some(
                success
                    .to_string()
                    .replace("\\n", "\n")
                    .replace("\\t", "\t"),
            );
            self.logger.log_debug("Added success message to index")?;
        }

        if let Some(referrer) = &self.input.referrer {
            index.referrer = Some(referrer.to_string());
            self.logger.log_debug("Added referrer to index")?;
        }

        if let Some(keys) = &self.input.google_api_key {
            index.google_api_key = Some(keys.to_string());
            self.logger.log_debug("Added google api key to index")?;
        }

        if let Some(keys) = &self.input.one_fichier_keys {
            index.one_fichier_keys = Some(keys.to_owned());
            self.logger.log_debug("Added 1Fichier keys to index")?;
        }

        if let Some(headers) = &self.input.headers {
            index.headers = Some(headers.to_owned());
            self.logger.log_debug("Added headers to index")?;
        }

        if let Some(version) = &self.input.min_version {
            index.version = Some(version.to_owned());
            self.logger.log_debug("Added minimum version to index")?;
        }

        if let Some(theme) = &self.input.theme_blacklist {
            index.theme_blacklist = Some(theme.to_owned());
            self.logger.log_debug("Added theme blacklist to index")?;
        }

        if let Some(theme) = &self.input.theme_whitelist {
            index.theme_whitelist = Some(theme.to_owned());
            self.logger.log_debug("Added theme whitelist to index")?;
        }

        if let Some(error) = &self.input.theme_error {
            index.theme_error = Some(error.to_string());
            self.logger
                .log_debug("Added theme error message to index")?;
        }

        self.logger.log_info("Generated index successfully")?;

        Ok(index)
    }

    pub fn output_index(&self, index: Index) -> result::Result<()> {
        let json = serde_json::to_string(&index)?;
        let compression = self.input.compression.to_owned();
        let encryption = match self.input.public_key {
            None => EncryptionFlag::NoEncrypt,
            Some(_) => EncryptionFlag::Encrypt,
        };

        fs::write(
            &self.input.output_path,
            convert_to_tinfoil_format(
                json.as_str(),
                compression,
                encryption,
                self.input.public_key.to_owned(),
            )?,
        )?;

        self.logger.log_info(
            format!(
                "Finished writing {} to disk, using {} compression & {}encryption",
                &self
                    .input
                    .output_path
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap(),
                compression = match compression {
                    CompressionFlag::Off => "no".to_string(),
                    CompressionFlag::ZSTD | CompressionFlag::Zlib => {
                        compression.to_string()
                    }
                },
                encryptiom = match encryption {
                    EncryptionFlag::NoEncrypt => "no ",
                    EncryptionFlag::Encrypt => "",
                }
            )
            .as_str(),
        )?;

        Ok(())
    }

    pub fn share_file(&self, file_id: String, is_shared: &bool) {
        if !is_shared {
            self.gdrive.as_ref().unwrap().share_file(file_id.as_str());
        }
    }

    pub fn share_files(&self, files: Vec<ParsedFileInfo>) -> result::Result<()> {
        let pb = ProgressBar::new(files.len() as u64);

        pb.set_style(
            ProgressStyle::default_bar()
                .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {msg} {pos:>7}/{len:7} Files")
                .progress_chars("#>-"),
        );

        pb.set_message("Sharing");

        for file in files {
            self.share_file(file.id, &file.shared);
            pb.inc(1);
        }

        pb.finish_with_message("Finished Sharing");

        Ok(())
    }

    pub fn upload_index(&self) -> std::io::Result<(String, bool)> {
        let folder_id = &self.input.upload_folder_id;
        let input = self.input.output_path.as_path();

        let res = self
            .gdrive
            .as_ref()
            .unwrap()
            .upload_file(input, &self.input.upload_folder_id)
            .unwrap();

        self.logger.log_info(
            format!(
                "Uploaded Index to {}",
                destination = folder_id
                    .as_ref()
                    .unwrap_or("My Drive".to_string().borrow())
            )
            .as_str(),
        )?;

        Ok(res)
    }

    pub fn share_index(&self, file_id: String, is_shared: bool) -> std::io::Result<()> {
        self.share_file(file_id, &is_shared);
        self.logger.log_info("Shared Index File")
    }

    pub fn scan_folder(&mut self) -> result::Result<Vec<ParsedFileInfo>> {
        let re = Regex::new("%5B[0-9A-Fa-f]{16}%5D")?;

        // Trigger Authentication if needed
        self.gdrive.as_ref().unwrap().trigger_auth();

        let pb = ProgressBar::new(!0);
        pb.enable_steady_tick(130);
        pb.set_style(
            ProgressStyle::default_spinner()
                // For more spinners check out the cli-spinners project:
                // https://github.com/sindresorhus/cli-spinners/blob/master/spinners.json
                .tick_strings(&["-", "\\", "|", "/"])
                .template("{spinner:.blue} {msg}"),
        );
        pb.set_message("Scanning...");

        let files: Vec<ParsedFileInfo> = self
            .input
            .folder_ids
            .iter()
            .map(|id| -> Vec<ParsedFileInfo> {
                self.gdrive
                    .as_ref()
                    .unwrap()
                    .get_all_files_in_folder(id.to_owned().as_str(), !self.input.no_recursion)
                    .unwrap()
                    .into_iter()
                    .map(|file_info| ParsedFileInfo::new(file_info))
                    .filter(|file| {
                        let mut keep = true;

                        if !self.input.add_non_nsw_files {
                            let extension: String = file
                                .name
                                .chars()
                                .skip(file.name.len() - 4)
                                .take(4)
                                .collect();

                            keep = vec![".nsp", ".nsz", ".xci", ".xcz"].contains(&&*extension);
                        }

                        if !self.input.add_nsw_files_without_title_id {
                            keep = re.is_match(file.name_encoded.as_str());
                        }

                        keep
                    })
                    .collect()
            })
            .flatten()
            .collect();

        pb.finish_with_message(&*format!("Scanned {} files", files.len()));

        Ok(files)
    }

    pub fn finalize(&self) -> std::io::Result<()> {
        self.logger
            .log_info(format!("Execution took {}", self.timer.elapsed().hhmmss()).as_str())
    }
}

pub fn main() {
    match real_main() {
        Ok(_) => std::process::exit(0),
        Err(_) => std::process::exit(1),
    }
}

fn real_main() -> result::Result<()> {
    let mut service = RustfoilService::new(Input::from_args());

    service.validate_input()?;

    service.init();

    let files = service.scan_folder()?;

    let index = service.generate_index(files.to_owned())?;

    service.output_index(*index)?;

    if service.input.share_files {
        service.share_files(files)?;
    }

    if service.input.upload_my_drive || service.input.upload_folder_id.is_some() {
        let (id, shared) = service.upload_index()?;

        if service.input.share_index {
            service.share_index(id, shared)?;
        }
    };

    service.finalize()?;

    Ok(())
}
