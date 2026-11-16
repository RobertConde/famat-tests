# Converting Originals to PDF headlessly in a container

Research for [#3](https://github.com/RobertConde/famat-tests/issues/3) (parent map [#1](https://github.com/RobertConde/famat-tests/issues/1)). Researched 2026-11-16 against primary sources: official docs, project source code, and issue trackers. Terms follow `GLOSSARY.md`: an **Original** is a Document file exactly as uploaded; the PDF is made from it and kept alongside it.

The real format mix is not known yet; the inventory ticket will settle it. This note assumes old `.doc` (with Equation Editor 3.0 and MathType objects), `.docx` (with OMML equations), and possibly `.wpd`, `.rtf`, `.odt`, `.tex`, images, and PDFs that already exist.

## Recommendation

1. **Use LibreOffice for every word-processor format** (`.doc`, `.docx`, `.rtf`, `.odt`, `.wpd`). It is the only self-hostable engine that reads all of them and keeps page layout. Run it **one-shot per Document** (`soffice --headless --convert-to pdf`) from the Rust app, inside the app's own Podman image, built on Debian `libreoffice-writer-nogui`.
   - Give every job its own throwaway profile (`-env:UserInstallation=file:///tmp/lo-<job-id>`), copied from a template profile baked into the image.
   - Cap concurrency with a `tokio::sync::Semaphore`: start with 2 and tune by measurement.
   - Spawn the process in its own process group, kill the whole group on a hard timeout (start with 120 s), and then check that the output is a non-empty, parseable PDF.
2. **Images**: wrap them losslessly with `img2pdf` (Debian package `img2pdf`). **Existing PDFs**: pass them through unchanged; a PDF Original needs no generated PDF.
3. **`.tex`**: no TeX engine in v1 unless the inventory finds `.tex` Originals. If it does, add Tectonic with `--untrusted` and an offline (pre-cached) bundle. Otherwise those Documents get an Admin-uploaded PDF.
4. **Decide the equation path with a fixture test before launch**. The single biggest fidelity lever is the LibreOffice setting `MathTypeToMath`. Run a fixture set of about 20 real Originals both ways and compare against PDFs printed from Word. The tentative default is **off**, so that every legacy equation is drawn from the picture Word itself saved (see [Math fidelity](#math-fidelity)).
5. **Treat conversion as a background job that can fail.** Store the Original first and record a PDF status per Document (`pending` / `ok` / `failed`). A failure never blocks an upload. Admins can replace any generated PDF with one they upload, which covers `.wpd` equations, `.tex`, and anything else LibreOffice gets wrong.

**Alternative if you'd rather not own the process supervision**: run Gotenberg's `gotenberg/gotenberg:9-libreoffice` image as a second container in the same Podman pod, and call it over HTTP from Rust with `reqwest` multipart. It is the same LibreOffice engine, plus crash retries and automatic restarts. In exchange you get a second container, conversions serialized per container, and no flag for import-filter options such as `MathTypeToMath`. See [Gotenberg](#gotenberg).

**Rejected**: Pandoc as the main converter, because it can't read `.doc` or `.wpd`, its output is lossy by design, and it drops page layout. Also rejected: Microsoft Graph's conversion API, because it isn't self-hosted, needs a Microsoft 365 tenant, and caps files at 2 MB.

## Comparison

| | LibreOffice (one-shot `soffice`) | unoserver | Gotenberg 9 | Pandoc (+ LaTeX) | TeX engine | MS Graph `?format=pdf` |
|---|---|---|---|---|---|---|
| Reads `.doc` / `.docx` / `.rtf` / `.odt` / `.wpd` | All (wpd via libwpd) | All (it is LibreOffice) | All (it is LibreOffice) | docx, rtf, odt only; no doc, no wpd | `.tex` only | doc, docx, rtf, odt; no wpd |
| Equation Editor 3.0 | Converted to LO Math, or Word's saved picture | same as LO | same as LO | Saved picture only (often WMF) | n/a | Native (it is Word) |
| MathType 4+ | Word's saved picture | same | same | Saved picture only | n/a | Native |
| OMML (`.docx`) | Converted to LO Math | same | same | Converted via texmath, relaid out in LaTeX | n/a | Native |
| Page layout kept | Yes (approximate) | Yes | Yes | No ("lossy") | n/a | Yes |
| Image size (compressed) | LO + fonts in app image (not measured) | LO + Python | ~375 MB (`9-libreoffice`), ~640 MB (full `9`) | ~241 MB (`pandoc/latex`) | ~2.8 GB (`texlive/texlive`) | none |
| Concurrency | One process per job, separate profiles | One conversion per server; run N servers on N ports | One at a time per container | Per process | Per process | Remote |
| Self-hosted | Yes | Yes | Yes | Yes | Yes | **No** |

Sizes are compressed `amd64` sizes from the Docker Hub API on 2026-11-16 (`gotenberg/gotenberg:9-libreoffice` 375 MB, `:9` 640 MB; `pandoc/latex:latest-ubuntu` 241 MB; `texlive/texlive:latest` 2,787 MB). Speed was not benchmarked here. Measure it on the fixture set, and note that one-shot `soffice` pays LibreOffice's start-up cost on every Document.

## Math fidelity

This is the top concern, and it hinges on how LibreOffice handles embedded equation objects.

### Equation Editor 3.0 (old `.doc` and `.docx`)

- LibreOffice converts an Equation Editor OLE object into a native LibreOffice Math object when the `MathTypeToMath` import option is on. **The option is on by default**: `officecfg/.../Common.xcs` declares `MathTypeToMath` with `<value>true</value>` ("Specifies if convertible MathType OLE objects are converted into Math objects") ([Common.xcs](https://github.com/LibreOffice/core/blob/master/officecfg/registry/schema/org/openoffice/Office/Common.xcs)).
- For `.doc`, the conversion applies only to objects whose OLE class is Equation Editor 2 or 3 (`MSO_EQUATION3_CLASSID`, `MSO_EQUATION2_CLASSID`) ([msdffimp.cxx `CheckForConvertToSOObj`](https://github.com/LibreOffice/core/blob/master/filter/source/msfilter/msdffimp.cxx)). For `.docx`, it applies only to ProgID `Equation.3`; any other ProgID is logged as "unhandled" ([OLEHandler.cxx](https://github.com/LibreOffice/core/blob/master/sw/source/writerfilter/dmapper/OLEHandler.cxx)).
- The MTEF parser accepts only format version 3 or lower: `if (!pS->good() || nVersion > 3) // allow only supported versions of MathType to be parsed return false;` ([mathtype.cxx](https://github.com/LibreOffice/core/blob/master/starmath/source/mathtype.cxx)). The help page agrees: embedded MathType objects "must not exceed the MathType 3.1 specifications" ([Help: Load/Save – Microsoft Office](https://help.libreoffice.org/latest/en-US/text/shared/optionen/01130200.html)).
- **What this means**: Equation Editor 3.0 equations are re-typeset by LibreOffice Math, so spacing and glyphs can differ from what Word showed. A converted equation is rebuilt from the MTEF data, not copied from Word's rendering. That rebuild is a likely source of small errors (e.g. a wrong embellishment or a misplaced limit), and only a fixture test will show how often it happens.

### MathType 4 and later (`Equation.DSMT4` and similar)

These are never converted (wrong class/ProgID, MTEF > 3). LibreOffice keeps them as foreign OLE objects and draws the replacement graphic that Word saved in the file (`aObj.SetGraphic(rGrf, ...)` in the import path, [msdffimp.cxx](https://github.com/LibreOffice/core/blob/master/filter/source/msfilter/msdffimp.cxx)). That picture is what Word displayed when the file was last saved, so fidelity is good as long as the picture's fonts render.

### Why "off" is the tentative default

With `MathTypeToMath=false`, Equation Editor 3.0 objects take the same path as MathType 4+ objects: they are drawn from Word's own saved picture instead of being re-typeset. That gives one consistent path for all legacy equations, and that path reproduces what the authors actually saw. The risk is that the saved pictures are WMF/EMF files that call fonts by name (Symbol, MT Extra, Times New Roman). LibreOffice has recoding tables that map **Symbol** and **MT Extra** glyphs to OpenSymbol when those fonts are missing (`aMTExtraTab`, `{"mtextra", {aMTExtraTab, u"StarSymbol"_ustr, ...}}` in [fontcvt.cxx](https://github.com/LibreOffice/core/blob/master/unotools/source/misc/fontcvt.cxx)). This is why the fixture test must cover both settings before the choice is final. Set the option in the template profile's `registrymodifications.xcu` (path `org.openoffice.Office.Common/Filter/Microsoft/Import`, property `MathTypeToMath`).

### OMML (Word 2007+ `.docx` equations)

LibreOffice imports OMML into Math objects ([starmath/source/ooxmlimport.cxx](https://github.com/LibreOffice/core/blob/master/starmath/source/ooxmlimport.cxx)). The importer has known gaps for unusual characters, which it logs as `"Unknown m:chr in m:acc"` and `"Unknown m:nary chr"`. Pandoc also reads OMML (`readOMML` from texmath in [Docx/Parse.hs](https://github.com/jgm/pandoc/blob/main/src/Text/Pandoc/Readers/Docx/Parse.hs)), but then relays out the whole Document through LaTeX.

### WordPerfect (`.wpd`)

LibreOffice imports WordPerfect through libwpd. libwpd's feature page lists "Equations and math", "Embedded images" and "Positioned Objects (e.g.: text boxes)" under **Planned Features**, not supported ones ([libwpd features](https://libwpd.sourceforge.net/features.html)). That page may be out of date, but expect WordPerfect equations to be missing or wrong. Flag every `.wpd` conversion for Admin review.

### Pandoc on legacy equations

Pandoc's docx reader turns a `w:object` (an OLE equation) into an image of its `v:imagedata` preview, and does nothing else with it ([Docx/Parse.hs](https://github.com/jgm/pandoc/blob/main/src/Text/Pandoc/Readers/Docx/Parse.hs)). That preview is usually WMF, which a LaTeX PDF engine cannot embed without an extra conversion step. Pandoc's own manual warns that "conversions from formats more expressive than pandoc's Markdown can be expected to be lossy" and that it does not keep "formatting details such as margin size" ([MANUAL](https://github.com/jgm/pandoc/blob/main/MANUAL.txt)). It has no `.doc` or `.wpd` reader (input-format list, same source).

## Failure modes

### LibreOffice one-shot `soffice`

- **A second instance silently does nothing.** If another `soffice` is running with the same user profile, `--convert-to` hands the request to that instance and exits without producing output. The known fix is a separate profile per run via `-env:UserInstallation=...` ([tdf#37531](https://bugs.documentfoundation.org/show_bug.cgi?id=37531); the flag is documented in [Help: start parameters](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html)). **Per-job profiles are mandatory.**
- **Hangs and crashes on bad input.** The process can stall or die on corrupt or exotic files. That is why Gotenberg retries crashes up to 10 times, with the comment "LibreOffice crashes randomly" ([gotenberg api.go](https://github.com/gotenberg/gotenberg/blob/main/pkg/modules/libreoffice/api/api.go), citing [gotenberg#639](https://github.com/gotenberg/gotenberg/issues/639)). Defend with a hard timeout, a process-group kill (the `soffice` launcher starts a child `soffice.bin`, so killing only the launcher can leave the child running), and at most one or two retries.
- **Wrong output that still succeeds.** The Document converts, but fonts are substituted or equations are re-typeset. Install metric-compatible fonts (`fonts-liberation`, `fonts-crosextra-carlito`, `fonts-crosextra-caladea`, plus the OpenSymbol that ships with LibreOffice) and rely on the fixture test plus Admin override.
- **Macros.** Leave macros disabled (the default). Gotenberg also keeps them off by default because "a macro runs code inside the container" ([gotenberg libreoffice.go](https://github.com/gotenberg/gotenberg/blob/main/pkg/modules/libreoffice/api/libreoffice.go)).

### unoserver

- `--conversion-timeout` "terminates LibreOffice and exits" when a conversion runs too long, and `--stop-after` exits after N requests. Unoserver "does not try to restart LibreOffice if it crashes", so "service monitoring software should be set up to restart Unoserver when it exits." Each instance needs unique `--port`/`--uno-port` values, and there is no built-in load balancing. It must run under the same Python installation that LibreOffice uses ([unoserver README](https://github.com/unoconv/unoserver)).
- That means a Python runtime plus a supervisor, for little gain over one-shot `soffice` at this archive's volume. Not recommended.

### Gotenberg

- It converts **one Document at a time** per container: "lok converts one document at a time" (a `convMu` mutex). On timeout it kills the LibreOffice process because "LibreOfficeKit calls cannot be cancelled", and on a crash it restarts and retries ([libreoffice.go](https://github.com/gotenberg/gotenberg/blob/main/pkg/modules/libreoffice/api/libreoffice.go), [api.go](https://github.com/gotenberg/gotenberg/blob/main/pkg/modules/libreoffice/api/api.go)).
- Defaults: `API_TIMEOUT` 30 s, `libreoffice-restart-after` 10 conversions, `libreoffice-max-queue-size` 0 (unbounded), `libreoffice-start-timeout` 20 s ([configuration docs](https://gotenberg.dev/docs/configuration); flag definitions in api.go).
- Its accepted extensions include `.doc`, `.docx`, `.rtf`, `.odt`, `.wpd`, `.wps`, and images (`.png`, `.jpg`, `.tif`, ...) (`Extensions()` in api.go). The endpoint is `POST /forms/libreoffice/convert` (multipart) ([convert docs](https://gotenberg.dev/docs/convert-with-libreoffice/convert-to-pdf)).
- None of its LibreOffice flags exposes import-filter settings such as `MathTypeToMath`, so changing that setting would require a derived image. The latest release is v9.5.1 (2026-11-14).

### TeX (only if `.tex` Originals exist)

- TeX can stop and wait for input on errors, and can run shell commands if `\write18` is enabled. Tectonic keeps shell-escape off by default, `--untrusted` "disable[s] all known-insecure features", and `-C/--only-cached` restricts it to locally cached files so builds work offline ([Tectonic compile docs](https://tectonic-typesetting.github.io/book/latest/v2cli/compile.html)).
- A full TeX Live image is ~2.8 GB compressed, which is a strong reason to skip TeX unless the inventory finds `.tex` Originals.

## How Rust calls it

Recommended path (one-shot `soffice` inside the app container):

```rust
// Sketch only; not an implementation.
let _permit = semaphore.acquire().await?;           // e.g. 2 concurrent jobs
let profile = tempdir_from_template("/opt/lo-profile-template")?;
let out = tempdir()?;
let mut cmd = tokio::process::Command::new("soffice");
cmd.args(["--headless", "--norestore", "--nologo", "--nolockcheck",
          &format!("-env:UserInstallation=file://{}", profile.path().display()),
          "--convert-to", "pdf", "--outdir"])
   .arg(out.path()).arg(&original_path)
   .process_group(0)                                 // own group, so we can kill soffice.bin too
   .kill_on_drop(true);
let child = cmd.spawn()?;
match tokio::time::timeout(Duration::from_secs(120), child.wait_with_output()).await {
    Ok(Ok(o)) if o.status.success() => verify_pdf(out.path())?,   // exists, non-empty, parses (e.g. lopdf)
    Ok(_) | Err(_) => { killpg(child_pgid, SIGKILL); mark_failed(doc_id) }
}
```

Notes:

- Copy the input to a temp file with a safe, generated name, so the Original's filename never reaches the command line as an option.
- The output name is the input stem plus `.pdf` in `--outdir`.
- Exit status alone is not trusted, because of the "silently does nothing" case: always check that the output file exists.
- Run the ~8,500-Document backfill through the same job queue.

Gotenberg alternative: `reqwest::multipart::Form` with a `files` part, sent as `POST http://localhost:3000/forms/libreoffice/convert`. The response body is the PDF. Treat 5xx responses and timeouts as `failed`.

## Container sketch (recommended path)

The app image is based on Debian trixie and adds:

- `libreoffice-writer-nogui` (trixie has version 25.2.3; [packages.debian.org](https://packages.debian.org/trixie/libreoffice-writer-nogui), "word processor (no GUI variant)")
- `img2pdf`
- Fonts: `fonts-liberation`, `fonts-crosextra-carlito`, `fonts-crosextra-caladea`, `fonts-dejavu`
- A pre-seeded LibreOffice profile template at `/opt/lo-profile-template` holding the chosen `MathTypeToMath` setting

`img2pdf` "directly embeds the image data into the PDF without re-encoding it" for JPEG, and the result keeps "the exact same color information for every pixel as the input" ([img2pdf](https://gitlab.mister-muffin.de/josch/img2pdf)).

## Open questions for the fixture test (before the spec is final)

1. `MathTypeToMath` on vs off: which matches Word-printed PDFs better for Equation Editor 3.0 Originals?
2. Is per-Document start-up time acceptable for the backfill (one-shot `soffice` vs a Gotenberg sidecar)?
3. How do `.wpd` Originals with equations come out? Is "Admin uploads a PDF" enough for them?
4. Are fonts missing in the container that the Originals use (check the PDF's font list with `pdffonts`)?

## Sources

- LibreOffice core source: [Common.xcs](https://github.com/LibreOffice/core/blob/master/officecfg/registry/schema/org/openoffice/Office/Common.xcs), [msdffimp.cxx](https://github.com/LibreOffice/core/blob/master/filter/source/msfilter/msdffimp.cxx), [mathtype.cxx](https://github.com/LibreOffice/core/blob/master/starmath/source/mathtype.cxx), [OLEHandler.cxx](https://github.com/LibreOffice/core/blob/master/sw/source/writerfilter/dmapper/OLEHandler.cxx), [ooxmlimport.cxx](https://github.com/LibreOffice/core/blob/master/starmath/source/ooxmlimport.cxx), [fontcvt.cxx](https://github.com/LibreOffice/core/blob/master/unotools/source/misc/fontcvt.cxx)
- LibreOffice Help: [Load/Save – Microsoft Office](https://help.libreoffice.org/latest/en-US/text/shared/optionen/01130200.html), [Start parameters](https://help.libreoffice.org/latest/en-US/text/shared/guide/start_parameters.html)
- LibreOffice Bugzilla: [tdf#37531](https://bugs.documentfoundation.org/show_bug.cgi?id=37531)
- unoserver: [README](https://github.com/unoconv/unoserver)
- Gotenberg: [configuration](https://gotenberg.dev/docs/configuration), [convert with LibreOffice](https://gotenberg.dev/docs/convert-with-libreoffice/convert-to-pdf), source [api.go](https://github.com/gotenberg/gotenberg/blob/main/pkg/modules/libreoffice/api/api.go) and [libreoffice.go](https://github.com/gotenberg/gotenberg/blob/main/pkg/modules/libreoffice/api/libreoffice.go), [Docker Hub tags](https://hub.docker.com/r/gotenberg/gotenberg/tags)
- Pandoc: [MANUAL.txt](https://github.com/jgm/pandoc/blob/main/MANUAL.txt), [Docx/Parse.hs](https://github.com/jgm/pandoc/blob/main/src/Text/Pandoc/Readers/Docx/Parse.hs)
- libwpd: [features](https://libwpd.sourceforge.net/features.html)
- Tectonic: [compile CLI](https://tectonic-typesetting.github.io/book/latest/v2cli/compile.html)
- img2pdf: [project page](https://gitlab.mister-muffin.de/josch/img2pdf)
- Microsoft Graph: [Convert to other formats](https://learn.microsoft.com/en-us/graph/api/driveitem-get-content-format) (2 MB limit; `.wpd` not listed)
- Debian: [libreoffice-writer-nogui (trixie)](https://packages.debian.org/trixie/libreoffice-writer-nogui)
