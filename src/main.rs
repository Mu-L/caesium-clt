use crate::options::{CommandLineArgs, OverwritePolicy};
use crate::scan_files::{get_file_mime_type, scan_files};
use caesium::compress_in_memory;
use caesium::parameters::CSParameters;
use clap::Parser;
use filetime::{set_file_times, FileTime};
use indicatif::{ParallelProgressIterator, ProgressBar, ProgressDrawTarget, ProgressStyle};
use rayon::iter::IntoParallelRefIterator;
use rayon::iter::ParallelIterator;
use std::error::Error;
use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::num::NonZero;
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::{fs, io};
use human_bytes::human_bytes;

mod logger;
mod options;
mod scan_files;

enum CompressionStatus {
    Success,
    Skipped,
    Error,
}

struct CompressionResult {
    original_path: String,
    output_path: String,
    original_size: u64,
    compressed_size: u64,
    status: CompressionStatus,
    message: String,
}

fn main() {
    let args = CommandLineArgs::parse();

    let quiet = args.quiet || args.verbose == 0;
    let threads_number = get_parallelism_count(
        args.threads,
        std::thread::available_parallelism()
            .unwrap_or(NonZero::new(1).unwrap())
            .get(),
    );
    let verbose = if quiet { 0 } else { args.verbose };
    let needs_resize = args.resize.width.is_some()
        || args.resize.height.is_some()
        || args.resize.long_edge.is_some()
        || args.resize.short_edge.is_some();
    let (base_path, input_files) = scan_files(&args.files, args.recursive, quiet);

    rayon::ThreadPoolBuilder::new()
        .num_threads(threads_number)
        .build_global()
        .unwrap_or_default();

    let total_files = input_files.len();

    let progress_bar = setup_progress_bar(total_files, verbose);
    let compression_results: Vec<CompressionResult> = input_files
        .par_iter()
        .progress_with(progress_bar)
        .map(|input_file| {
            let mut compression_result = CompressionResult {
                original_path: input_file.display().to_string(),
                output_path: String::new(),
                original_size: 0,
                compressed_size: 0,
                status: CompressionStatus::Error,
                message: String::new(),
            };

            let input_file_metadata = match input_file.metadata() {
                Ok(m) => m,
                Err(_) => {
                    compression_result.message = "Error reading file metadata".to_string();
                    return compression_result;
                }
            };
            let original_file_size = input_file_metadata.len();
            compression_result.original_size = original_file_size;

            let output_directory = if args.output_destination.same_folder_as_input {
                match input_file.parent() {
                    Some(p) => p,
                    None => {
                        compression_result.message = "Error getting parent directory".to_string();
                        return compression_result;
                    }
                }
            } else {
                match args.output_destination.output.as_ref() {
                    Some(p) => p,
                    None => {
                        compression_result.message = "Error getting output directory".to_string();
                        return compression_result;
                    }
                }
            };

            let output_full_path = match compute_output_full_path(
                output_directory.to_path_buf(),
                input_file.to_path_buf(),
                base_path.to_path_buf(),
                args.keep_structure,
                args.suffix.as_ref().unwrap_or(&String::new()).as_ref(),
            ) {
                Some(p) => p,
                None => {
                    compression_result.message = "Error computing output path".to_string();
                    return compression_result;
                }
            };

            if args.dry_run {
                compression_result.status = CompressionStatus::Success;
                return compression_result;
            };

            let compression_parameters = build_compression_parameters(&args, input_file, needs_resize);

            let compressed_image =
                match compress_in_memory(read_file_to_vec(input_file).unwrap(), &compression_parameters) {
                    //TODO: handle error
                    Ok(v) => v,
                    Err(e) => {
                        compression_result.message = format!("Error compressing file: {}", e);
                        return compression_result;
                    }
                };
            compression_result.output_path = output_full_path.display().to_string();
            let output_file_size = compressed_image.len() as u64;

            if output_full_path.exists() {
                match args.overwrite {
                    OverwritePolicy::None => {
                        compression_result.status = CompressionStatus::Skipped;
                        compression_result.compressed_size = original_file_size;
                        compression_result.message = "File already exists, skipped due overwrite policy".to_string();
                        return compression_result;
                    }
                    OverwritePolicy::Bigger => {
                        if output_file_size >= original_file_size {
                            compression_result.status = CompressionStatus::Skipped;
                            compression_result.compressed_size = original_file_size;
                            compression_result.message =
                                "File already exists, skipped due overwrite policy".to_string();
                            return compression_result;
                        }
                    }
                    _ => {}
                }
            }

            let mut output_file = match File::create(&output_full_path) {
                Ok(f) => f,
                Err(_) => {
                    compression_result.message = "Error creating output file".to_string();
                    return compression_result;
                }
            };
            match output_file.write_all(&compressed_image) {
                Ok(_) => {}
                Err(_) => {
                    compression_result.message = "Error writing output file".to_string();
                    return compression_result;
                }
            };

            if args.keep_dates {
                let (last_modification_time, last_access_time) = (
                    FileTime::from_last_modification_time(&input_file_metadata),
                    FileTime::from_last_access_time(&input_file_metadata),
                );
                match preserve_dates(&output_full_path, last_modification_time, last_access_time) {
                    Ok(_) => {}
                    Err(_) => {
                        compression_result.message = "Error preserving file dates".to_string();
                        return compression_result;
                    }
                }
            }

            compression_result.status = CompressionStatus::Success;
            compression_result.compressed_size = output_file_size;
            compression_result
        })
        .collect();
    
    write_recap_message(&compression_results, verbose);
}

fn write_recap_message(compression_results: &[CompressionResult], verbose: u8) {
    let mut total_original_size = 0;
    let mut total_compressed_size = 0;
    let total_files = compression_results.len();
    let mut total_success = 0;
    let mut total_skipped = 0;
    let mut total_errors = 0;

    for result in compression_results.iter() {
        total_original_size += result.original_size;
        total_compressed_size += result.compressed_size;
        match result.status {
            CompressionStatus::Skipped => total_skipped += 1,
            CompressionStatus::Error => total_errors += 1,
            _ => total_success += 1
        }
    }

    let total_saved = total_original_size as f64 - total_compressed_size as f64;
    let total_saved_percent = total_saved / total_original_size as f64 * 100.0;

    if verbose > 0 {
        println!("Total files: {}", total_files);
        println!("Total success: {}", total_success);
        println!("Total skipped: {}", total_skipped);
        println!("Total errors: {}", total_errors);
        println!("Total original size: {}", human_bytes(total_original_size as f64));
        println!("Total compressed size: {}", human_bytes(total_compressed_size as f64));
        println!("Total saved: {:.2} bytes ({:.2}%)", human_bytes(total_saved), total_saved_percent);
    }
}

fn get_parallelism_count(requested_threads: u32, available_threads: usize) -> usize {
    if requested_threads > 0 {
        std::cmp::min(available_threads, requested_threads as usize)
    } else {
        available_threads
    }
}

fn build_compression_parameters(args: &CommandLineArgs, input_file: &Path, needs_resize: bool) -> CSParameters {
    let mut parameters = CSParameters::new();
    let quality = args.compression.quality.unwrap_or(80) as u32;

    parameters.jpeg.quality = quality;
    parameters.png.quality = quality;
    parameters.webp.quality = quality;
    parameters.gif.quality = quality;

    parameters.keep_metadata = args.exif;

    parameters.png.optimization_level = args.png_opt_level;
    parameters.png.force_zopfli = args.zopfli;

    if needs_resize {
        let mime_type = get_file_mime_type(input_file);
        build_resize_parameters(args, &mut parameters, input_file, mime_type).unwrap();
        //TODO
    }

    parameters
}

fn build_resize_parameters(
    args: &CommandLineArgs,
    parameters: &mut CSParameters,
    input_file_path: &Path,
    mime_type: Option<String>,
) -> Result<(), Box<dyn Error>> {
    let (width, height) = get_real_resolution(input_file_path, mime_type, args.exif)?;

    if args.resize.width.is_some() {
        parameters.width = args.resize.width.unwrap_or(0);
    } else if args.resize.height.is_some() {
        parameters.height = args.resize.height.unwrap_or(0);
    } else if args.resize.long_edge.is_some() {
        let long_edge = args.resize.long_edge.unwrap_or(0);
        if width > height {
            parameters.width = long_edge;
        } else {
            parameters.height = long_edge;
        }
    } else if args.resize.short_edge.is_some() {
        let short_edge = args.resize.short_edge.unwrap_or(0);
        if width < height {
            parameters.width = short_edge;
        } else {
            parameters.height = short_edge;
        }
    }

    Ok(())
}

fn compute_output_full_path(
    output_directory: PathBuf,
    input_file_path: PathBuf,
    base_directory: PathBuf,
    keep_structure: bool,
    suffix: &str,
) -> Option<PathBuf> {
    let extension = input_file_path.extension().unwrap_or_default().to_os_string();
    let base_name = input_file_path.file_stem().unwrap_or_default().to_os_string();
    let mut output_file_name = base_name;
    output_file_name.push(suffix);
    if !extension.is_empty() {
        output_file_name.push(".");
        output_file_name.push(extension);
    }

    if keep_structure {
        let parent = match input_file_path.parent()?.canonicalize() {
            Ok(p) => p,
            Err(_) => return None,
        };

        let output_path_prefix = match parent.strip_prefix(base_directory) {
            Ok(p) => p,
            Err(_) => return None,
        };
        let full_output_directory = output_directory.join(output_path_prefix);
        fs::create_dir_all(&full_output_directory).ok()?; // TODO I don't like that the creation is done inside this function because the name is a bit obscure
        Some(full_output_directory.join(output_file_name))
    } else {
        fs::create_dir_all(&output_directory).ok()?; // TODO I don't like that the creation is done inside this function because the name is a bit obscure
        Some(output_directory.join(output_file_name))
    }
}

fn read_file_to_vec(file_path: &PathBuf) -> io::Result<Vec<u8>> {
    let mut file = File::open(file_path)?;
    let mut buffer = Vec::new();
    file.read_to_end(&mut buffer)?;
    Ok(buffer)
}

fn preserve_dates(output_file: &PathBuf, input_atime: FileTime, input_mtime: FileTime) -> io::Result<()> {
    set_file_times(output_file, input_atime, input_mtime)
}

fn get_real_resolution(
    file: &Path,
    mime_type: Option<String>,
    keep_metadata: bool,
) -> Result<(usize, usize), Box<dyn Error>> {
    let resolution = imagesize::size(file)?;
    let mut orientation = 1;
    let mime = mime_type.unwrap_or("".to_string());
    if mime == "image/jpeg" && keep_metadata {
        let f = File::open(file)?;
        if let Ok(e) = exif::Reader::new().read_from_container(&mut BufReader::new(&f)) {
            let exif_field = match e.get_field(exif::Tag::Orientation, exif::In::PRIMARY) {
                Some(f) => f,
                None => return Ok((resolution.width, resolution.height)),
            };
            orientation = exif_field.value.get_uint(0).unwrap_or(1);
        };
    }
    let (width, height) = match orientation {
        5..=8 => (resolution.height, resolution.width),
        _ => (resolution.width, resolution.height),
    };

    Ok((width, height))
}

fn setup_progress_bar(len: usize, verbose: u8) -> ProgressBar {
    let progress_bar = ProgressBar::new(len as u64);
    if verbose == 0 {
        progress_bar.set_draw_target(ProgressDrawTarget::hidden());
    } else {
        progress_bar.set_draw_target(ProgressDrawTarget::stdout());
    }
    progress_bar.set_style(
        ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len}\n{msg}")
            .unwrap() //TODO: handle error
            .progress_chars("#>-"),
    );
    progress_bar.enable_steady_tick(Duration::new(1, 0));
    progress_bar
}

#[cfg(test)] 
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_get_parallelism_count() {
        let result = get_parallelism_count(4, 4);
        assert_eq!(result, 4);

        let result = get_parallelism_count(2, 8);
        assert_eq!(result, 2);

        let result = get_parallelism_count(0, 8);
        assert_eq!(result, 8);

        let result = get_parallelism_count(1, 8);
        assert_eq!(result, 1);

        let result = get_parallelism_count(8, 2);
        assert_eq!(result, 2);

        let result = get_parallelism_count(0, 0);
        assert_eq!(result, 0);
    }

    #[test]
    fn test_compute_output_full_path() {
        let output_directory = PathBuf::from("/output");
        let base_directory = PathBuf::from("/base");

        // Test case 1: keep_structure = true
        let input_file_path = PathBuf::from("/base/folder/test.jpg");
        let result = compute_output_full_path(
            output_directory.clone(),
            input_file_path.clone(),
            base_directory.clone(),
            true,
            "_suffix",
        )
        .unwrap();
        assert_eq!(result, Path::new("/output/folder/test_suffix.jpg"));

        // Test case 2: keep_structure = false
        let result = compute_output_full_path(
            output_directory.clone(),
            input_file_path.clone(),
            base_directory.clone(),
            false,
            "_suffix",
        )
        .unwrap();
        assert_eq!(result, Path::new("/output/test_suffix.jpg"));

        // Test case 3: input file without extension
        let input_file_path = PathBuf::from("/base/folder/test");
        let result = compute_output_full_path(
            output_directory.clone(),
            input_file_path.clone(),
            base_directory.clone(),
            false,
            "_suffix",
        )
        .unwrap();
        assert_eq!(result, Path::new("/output/test_suffix"));

        // Test case 4: input file with different base directory
        let input_file_path = PathBuf::from("/different_base/folder/test.jpg");
        let result = compute_output_full_path(
            output_directory.clone(),
            input_file_path.clone(),
            base_directory.clone(),
            false,
            "_suffix",
        )
        .unwrap();
        assert_eq!(result, Path::new("/output/test_suffix.jpg"));
    }
}
