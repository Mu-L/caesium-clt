use std::path::PathBuf;
use clap::{Args, Parser, ValueEnum};

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, ValueEnum, Debug)]
pub enum OverwritePolicy {
    /// Always overwrite
    All,
    /// Never overwrite
    None,
    /// Overwrite only if the file to be overwritten is bigger
    Bigger
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct CommandLineArgs {
    #[command(flatten)]
    pub compression: Compression,
    
    #[command(flatten)]
    pub resize: Resize,

    #[command(flatten)]
    pub output_destination: OutputDestination,

    /// select level for PNG optimization, between [0-6]
    #[arg(long, default_value = "3")]
    pub png_opt_level: u8,

    /// use zopfli when optimizing PNG files (it may take a very long time to complete)
    #[arg(long)]
    pub zopfli: bool,
    
    /// keeps EXIF info during compression
    #[arg(short, long)]
    pub exif: bool,

    /// keep original file date information
    #[arg(long)]
    pub keep_dates: bool,

    /// add a suffix to the output filename
    #[arg(long)]
    pub suffix: Option<String>,

    /// if input is a folder, scan subfolders too
    #[arg(short = 'R', long)]
    pub recursive: bool,

    /// keep the folder structure, can be used only with -R
    #[arg(short = 'S', long)]
    pub keep_structure: bool,

    /// do not write output files
    #[arg(long, short, default_value = "false")]
    pub dry_run: bool,

    /// specify the number of parallel jobs (max is the number of processors available)
    #[arg(long, default_value = "0")]
    pub threads: u32,

    /// suppress all output
    #[arg(short = 'Q', long, group = "verbosity")]
    pub quiet: bool,

    /// overwrite policy
    #[arg(short = 'O', long, value_enum, default_value = "all")]
    pub overwrite: OverwritePolicy,

    /// select how much output you want to see, 0 is equal to -Q, --quiet
    #[arg(long, default_value = "1", group = "verbosity")]
    pub verbose: u8,

    pub files: Vec<String>,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct Compression {
    /// sets output file quality between [0-100]
    #[arg(short, long)]
    pub quality: Option<u8>,

    /// perform lossless compression
    #[arg(long, default_value = "false")]
    pub lossless: bool,

    /// set the expected maximum output size in bytes
    #[arg(long)]
    pub max_size: Option<u8>,
}

#[derive(Args, Debug)]
#[group(required = false, multiple = true)]
pub struct Resize {
    /// width of the output image, if height is not set will preserve aspect ratio
    #[arg(long, conflicts_with_all = &["long_edge", "short_edge"])]
    pub width: Option<u32>,

    /// height of the output image, if width is not set will preserve aspect ratio
    #[arg(long, conflicts_with_all = &["long_edge", "short_edge"])]
    pub height: Option<u32>,

    /// sets the size of the longest edge of the image
    #[arg(long, conflicts_with_all = &["width", "height", "short_edge"])]
    pub long_edge: Option<u32>,

    /// sets the size of the shortest edge of the image
    #[arg(long, conflicts_with_all = &["width", "height", "long_edge"])]
    pub short_edge: Option<u32>,
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct OutputDestination {
    /// output folder
    #[arg(short = 'o', long, group = "output_destination")]
    pub output: Option<PathBuf>,

    /// sets the output folder to be the same as the input folder, overwrites original files
    #[arg(long, default_value = "false", group = "output_destination")]
    pub same_folder_as_input: bool,
}