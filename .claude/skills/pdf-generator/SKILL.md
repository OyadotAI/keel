---
name: pdf-generator
metadata:
  generated-by: keel
description: "Use when the user wants to turn plain text (notes, a report, a letter, generated content) into a PDF file saved on disk."
---

# PDF Generator

Writes plain text, with an optional bold title, into a paginated US Letter PDF. Uses only the Python standard library.

## How to run

```
python3 .claude/skills/pdf-generator/scripts/run.py '{"text": "First paragraph.\n\nSecond paragraph goes here.", "title": "Weekly Report", "output_path": "out/report.pdf"}'
```

Inputs:
- `text` (required): the body text. Newlines are kept, and long lines wrap automatically.
- `title` (optional): a bold heading on page 1, also saved as the PDF's document title.
- `output_path` (optional, default `document.pdf`): where the file is written. Missing parent folders are created.
- `font_size` (optional, 6–72, default 11).

Output is one line of JSON:

```
{"path": "/abs/path/out/report.pdf", "pages": 1, "bytes": 1432}
```

On failure it prints `{"error": "..."}` and exits with code 1.

## Environment variables

None.

## When to use

Use this skill when the user asks for text to be saved as a PDF. If the user wants a summary or rewrite, write that text yourself first, then pass the finished text in. The skill uses plain Helvetica only, so there are no images, tables, or Markdown styling. Characters outside Latin-1 (such as emoji or CJK) show up as `?`, so tell the user about this if their text contains them. Afterwards, report the returned `path` to the user.
