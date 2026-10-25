use crate::Result;
use plotters::prelude::*;
use std::path::Path;

pub fn init() -> Result<()> {
    plotters::style::register_font(
        "sans-serif",
        FontStyle::Normal,
        include_bytes!("../assets/DroidSans.ttf"),
    )
    .map_err(|_| anyhow::anyhow!("Embedded font registration failed"))?;
    Ok(())
}

pub fn bars(path: &Path, title: &str, values: &[(String, usize)]) -> Result<()> {
    let note = "Estimated tokens: content size only";
    // Compare exact rendering inputs so unchanged charts keep their files and mtime.
    let inputs_path = path.with_extension("inputs.json");
    let inputs = serde_json::to_vec(&serde_json::json!({
        "renderer": 1, "title": title, "note": note, "values": values
    }))?;
    if path.is_file() && std::fs::read(&inputs_path).ok().as_deref() == Some(&inputs) {
        return Ok(());
    }
    let tmp = path.with_file_name(format!(
        ".{}.tmp.png",
        path.file_stem().unwrap_or_default().to_string_lossy()
    ));
    let root = BitMapBackend::new(&tmp, (1200, 650)).into_drawing_area();
    root.fill(&RGBColor(248, 250, 252))?;
    root.draw(&Text::new(
        title,
        (30, 38),
        ("sans-serif", 24).into_font().color(&RGBColor(15, 23, 42)),
    ))?;
    root.draw(&Text::new(note, (30, 70), ("sans-serif", 18).into_font()))?;
    let max = values.iter().map(|x| x.1).max().unwrap_or(1).max(1) as f64;
    if values.is_empty() {
        root.draw(&Text::new(
            "No observations yet",
            (30, 130),
            ("sans-serif", 22).into_font(),
        ))?;
    }
    for (i, (label, value)) in values.iter().take(10).enumerate() {
        let y = 110 + i as i32 * 48;
        root.draw(&Text::new(
            label.as_str(),
            (30, y + 23),
            ("sans-serif", 18).into_font(),
        ))?;
        let width = (*value as f64 / max * 650.) as i32;
        root.draw(&Rectangle::new(
            [(300, y), (950, y + 30)],
            RGBColor(226, 232, 240).filled(),
        ))?;
        root.draw(&Rectangle::new(
            [(300, y), (300 + width, y + 30)],
            RGBColor(59, 130, 246).filled(),
        ))?;
        root.draw(&Text::new(
            value.to_string(),
            (975, y + 23),
            ("sans-serif", 18).into_font(),
        ))?;
    }
    root.present()?;
    drop(root);
    std::fs::rename(tmp, path)?;
    crate::storage::atomic_write(&inputs_path, &inputs)?;
    Ok(())
}
