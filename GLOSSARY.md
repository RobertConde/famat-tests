# FAMAT Test Bank

An archive of past FAMAT and Mu Alpha Theta competition tests, searchable by their metadata and downloadable as documents.

## Language

### The archive

**Test**:
One exam given at one competition in one year, for one Division/Level and Test type.
_Avoid_: Exam, entry, row

**Published / Hidden**:
A Published Test is visible to everyone; a Hidden Test is visible only to Admins, e.g. while it is being catalogued.
_Avoid_: Draft, private, live

**Document**:
A single file belonging to a Test, such as its questions or its solutions.
_Avoid_: Doc, file, download

**Document kind**:
The role a Document plays for its Test, drawn from a fixed, admin-editable list (Test, Answers, Solutions, Condensed Test, Combined Test+Solutions, Errata, Other). Each kind states what it contains: questions, answers, solutions, or a mix (Solutions also contain answers).
_Avoid_: Doc title, document type

**Label source**:
Who assigned a Document's kind and its Test's facet values: the student cataloguing sheet, an AI labeling pass, or an Admin. AI labels are always marked as such.
_Avoid_: Provenance, author

**Needs review**:
A flag on a Document whose labels an Admin should check, with a free-text comment saying why.
_Avoid_: Unverified, pending

**Original**:
A Document file exactly as it was uploaded, kept alongside any PDF made from it.
_Avoid_: Source file

### Facets

**Facet**:
One of the six dimensions a Test is classified and searched by: Competition, Month, Division/Level, Test type, Host, Year. Each facet except Year has an admin-curated list of values in a chosen display order. Any facet may be unknown for a given Test.
_Avoid_: Category (for the dimensions in general), field, filter

**Competition**:
The kind of event a Test was given at: Invitational, Regional, State, National, or Mail-in.
_Avoid_: Category

**Month**:
The month of the competition, for events held more than once a season (Invitationals, Regionals). Not applicable to State, National, and Mail-in events, which is different from unknown.
_Avoid_: Date

**Division/Level**:
The course level or Mu Alpha Theta division a Test was written for, such as Algebra I, Geometry, Precalculus/Alpha, or Theta.
_Avoid_: Level, Division (alone)

**Test type**:
The format or topic of a Test within its Division/Level, such as Individual, Team, Ciphering, or a topic test like Logs and Exponents.
_Avoid_: Style

**Host**:
The school or organization that ran the event the Test was given at (FAMAT for State, MAO for National). The school that wrote a Test is not its Host.
_Avoid_: Writer, school

### Question bank (future; not built in v1)

**Question**:
One problem from a Test, traced to the Document and page it came from, with its answer choices, answer and solution. It keeps its original text exactly as printed and an edited version.
_Avoid_: Problem, item

**Attachment**:
An image, such as a figure cropped from a page, that belongs to a Question or Question group and is shown when it is rendered.
_Avoid_: Figure (for the stored file), asset

**Question group**:
A run of Questions that share context, such as "Use the figure below for 12–14."
_Avoid_: Passage, stimulus, section

**Dependency**:
A link stating that one Question builds on another, as in "Using your answer to #3…".
_Avoid_: Prerequisite

### People

**Admin**:
Someone who has the passcode and so can add Tests and curate Facet values.
_Avoid_: Editor, uploader
