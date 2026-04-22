#[cfg(test)]
use std::future::Future;

#[cfg(test)]
pub(crate) async fn list_files_with_extension(dir: &str, extension: &str) -> Vec<String> {
    let mut files = Vec::new();
    let mut entries = tokio::fs::read_dir(dir)
        .await
        .expect("failed to read fixtures directory");

    while let Some(entry) = entries
        .next_entry()
        .await
        .expect("failed to iterate fixtures directory")
    {
        let path = entry.path();
        let has_matching_extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case(extension));

        if path.is_file() && has_matching_extension {
            files.push(path.to_string_lossy().to_string());
        }
    }

    files.sort();
    files
}

#[cfg(test)]
pub(crate) async fn assert_all_files_with_extension_have_mime_type<Detect, DetectFuture>(
    path: &str,
    extension: &str,
    expected_mime: &str,
    skip_file_suffixes: &[&str],
    mut detect_mime: Detect,
) where
    Detect: FnMut(String) -> DetectFuture,
    DetectFuture: Future<Output = Result<String, crate::MimeTypeError>>,
{
    let files = list_files_with_extension(path, extension).await;
    assert!(
        !files.is_empty(),
        "expected at least one .{extension} fixture file"
    );

    for file in &files {
        if skip_file_suffixes
            .iter()
            .any(|suffix| file.ends_with(suffix))
        {
            continue;
        }

        let mime_type = detect_mime(file.to_string())
            .await
            .unwrap_or_else(|err| panic!("failed to detect MIME type for {file}: {err}"));
        assert_eq!(
            mime_type, expected_mime,
            "expected {expected_mime} for {file}, got {mime_type}"
        );
    }
}
