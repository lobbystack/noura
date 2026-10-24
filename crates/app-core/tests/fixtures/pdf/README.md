# PDF fixtures

The file viewer tests in `app-core` and `app-ui` open these PDFs.

| File | Pages | What it holds |
|---|---|---|
| `text.pdf` | 3 | Helvetica text that the search tests look for: "apple" once on page 1 and twice on page 3. |
| `large.pdf` | 400 | One dense drawing of 10,800 squares that every page shares, so a page takes a while to render. |
| `locked.pdf` | 2 | Text behind the user password `open sesame`, encrypted with the standard security handler, revision 3, 128-bit RC4. |

`make_fixtures.py` wrote all three with Python's standard library alone. To write them again:

```sh
python3 make_fixtures.py .
```

Poppler's `pdfinfo` and `pdftotext` read the same page counts and text, and they open `locked.pdf` only with `-upw "open sesame"`.
