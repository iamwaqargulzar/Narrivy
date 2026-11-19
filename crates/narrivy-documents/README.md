# narrivy-documents

Turns the assistant's Markdown into Word (`.docx`), PDF and PowerPoint
(`.pptx`) files with Narrivy's own code (D55): no LibreOffice, Office or
Python, no dependencies beyond the Rust standard library, the same output on
Linux and Windows.

- `parse` reads a Markdown subset: `#`–`###` headings, paragraphs, `-`/`*`
  and `1.` lists, `**bold**`, `*italic*`, `` `code` ``, fenced code blocks,
  `|` tables, `---` rules (a new slide in slides).
- `render(Format, title, markdown)` returns the file's bytes.
- `.docx`/`.pptx` are zipped Office Open XML (stored, CRC-32 checked).
- PDF uses the standard Helvetica and Courier fonts (Latin text). Text in
  other scripts (Urdu, Arabic) is kept in `.docx` and `.pptx`; the PDF
  writer replaces characters it cannot show with `?`.

Portable crate: no OS, UI or network code; `unsafe_code = forbid`.
