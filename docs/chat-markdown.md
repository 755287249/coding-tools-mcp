# Chat Markdown

The shared Desktop and Node UI uses Marked's lexer with CommonMark block/inline rules and GitHub Flavored Markdown (GFM). Tokens are rendered recursively as Svelte elements. References:

- https://marked.js.org/using_pro#lexer
- https://spec.commonmark.org/0.31.2/
- https://github.github.com/gfm/

Supported: headings 1–6 (including Setext), nested ordered/unordered/task lists, blockquotes, thematic breaks, emphasis/strong/strikethrough, inline/indented/fenced code, reference links, HTTP(S) autolinks, images and aligned tables. Chat prose preserves single line breaks. GFM table cells escape literal pipes with `\|`, including inside code spans.

Raw HTML is displayed literally. Link destinations are decoded before validation; only HTTP(S) destinations open externally. Existing workspace file links, image attachments, attachment mentions and robot mentions keep their application behavior. Remote images load lazily without a referrer. HTML/JavaScript code runs only through the separate explicit sandbox preview. Code copying preserves fenced source whitespace, including its trailing newline and unfinished streaming fences.

Navigation owns a shared summary poller for the lifetime of the shell. Mobile drawer contents remain mounted while closed. Opening a drawer does not clear the summary list, start a second poller, replace the transcript or navigate. Authentication/backend resets clear the navigation cache and discard old in-flight responses.

Project navigation and mobile swipes put live attached AI conversations first, then pins and actual message activity. Recent is strictly descending `last_message_at`, with a stable ID tie-break; changing metadata or renewing a lease never changes that timestamp. Discussion activity includes posts, member replies and summaries. Swipes exclude editors, controls, browser edges and horizontally scrollable content; they do not wrap at the first or last conversation.
