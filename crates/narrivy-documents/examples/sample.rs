//! Writes a sample document in every format to a folder, to open and
//! check by eye or with other tools: `cargo run -p narrivy-documents
//! --example sample -- <folder>`.

use narrivy_documents::{Format, render};

const SAMPLE: &str = "# Quarterly review\n\nThis report covers **sales**, *hiring* and the `release` plan.\n\n## Highlights\n\n- Revenue grew 18% on the quarter, the best since 2024.\n- Two new customers in Lahore and Karachi.\n- The NPU build shipped on time.\n\n## Next steps\n\n1. Hire two engineers.\n2. Finish the Windows port.\n3. Publish 1.0.\n\n| Region | Q3 | Q4 |\n|---|---|---|\n| North | 120 | 150 |\n| South | 90 | 130 |\n\n```\ncargo build --release\n```\n\n---\n\n## اردو\n\nیہ ایک مثال ہے۔\n";

fn main() -> std::io::Result<()> {
    let folder = std::env::args().nth(1).unwrap_or_else(|| ".".to_owned());
    for format in [Format::Docx, Format::Pdf, Format::Pptx, Format::Markdown] {
        let bytes = render(format, "Quarterly review", SAMPLE).map_err(std::io::Error::other)?;
        let path = std::path::Path::new(&folder).join(format!("sample.{}", format.extension()));
        std::fs::write(&path, bytes)?;
        println!("{}", path.display());
    }
    Ok(())
}
