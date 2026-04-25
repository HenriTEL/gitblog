# gitblog agent guide

## Goal
`gitblog` builds a static blog from Markdown stored in a Git repository. It fetches source files from a remote branch, updates post metadata, renders HTML pages, and emits an Atom feed.

## Project structure
- `src/main.rs`: CLI entrypoint; orchestrates fetch, incremental/full refresh, HTML generation, feed generation, and static assets copy.
- `src/lib.rs`: crate module exports and shared constants such as ignored source paths.
- `src/blog_post.rs`: blog post domain model and markdown-derived metadata updates.
- `src/feed.rs`: Atom feed parsing, merge/hydration logic, and XML generation.
- `src/git.rs`: all Git logic, including remote fetch, object decoding, tree diffing, and blob/path metadata indexing.
- `src/html.rs`: all HTML logic, including Markdown parsing with `comrak` and page generation with Tera templates.
- `src/templates.rs`: Tera environment setup and embedded template loading.
- `src/static_content.rs`: copies non-markdown site assets to output.
- `templates/`: Tera templates (`article.html.j2`, `index.html.j2`, partials).
- `css/`: stylesheet assets served by generated pages.
- `media/`: images and static media files.
- `tests/`: integration-style and behavior tests.

## Runtime flow (high level)
1. Parse CLI args (`repo`, `branch`, `blog_url`, `--full`).
2. Fetch existing Atom feed to determine incremental cutoff.
3. Pull changed files or full tree from remote Git repository.
4. Refresh post metadata from markdown and/or feed state.
5. Render markdown posts to HTML and build `index.html`.
6. Generate `atom.xml` and copy static assets when needed.

## Guardrails
- Keep this file concise and under 600 tokens.
- Update this guide whenever module ownership or top-level structure changes.
