# Confirm a signed-in website study

A saved browser sign-in may have expired, opened a public landing page, or used a different
account. Before the builder receives a study that used a saved sign-in, Craft Anything asks you
to check its first screenshots. Confirm only when the website and account are the ones you intend.
Confirmation permits the builder to use the captured site content, including any private account
information visible in it. It does not grant permission to publish, fetch owned assets, or perform
other actions; those keep their existing approvals.

On Make, the website sign-in row explains this check. Start studies the website, then the project
shows an approval card with local screenshots, the website, and Confirm and Reject. Returning
to Make while it waits shows that same card and answers the same inbox request. Tab and
Shift+Tab move between the buttons; Enter activates the focused action. A failed preview disables
Confirm and explains how to start a new study. Reject and Pause keep the builder from starting.
Previews are decoded on a worker from files inside this project's `capture/`; displaying them
makes no request to an AI service or website.

An attended `craft-anything-cli rebuild` lists the screenshot files in the project and asks you to
type `confirm`. Any other answer, EOF, or `--unattended` refuses. For a background rebuild started
through the existing automation tools, inspect its inbox with:

```sh
craft-anything-cli project-show --project "<project folder>"
```

The pending approval's `capture_review` names the website and screenshots relative to `capture/`,
with their hashes. Open those files locally. Then answer this request's id explicitly:

```sh
craft-anything-cli project-inbox-answer --project "<project folder>" --id "<request id>" --allow true
# Or reject it:
craft-anything-cli project-inbox-answer --project "<project folder>" --id "<request id>" --allow false
```

`always` is refused for capture approvals. Agent and MCP calls cannot approve account content on
the person's behalf; use the desktop or an independently started CLI. A CLI inherited from a
Craft Anything builder is marked as an agent and is refused as well.

The worker posts one request, waits for its answer, and rechecks the complete capture digest,
project target and saved-sign-in revision before preparing the builder. A missing answer times
out after 30 minutes; cancellation wins even if an answer has arrived. Resuming or starting a new
run asks again, so an earlier approval never authorizes another capture. A rejected capture or a
capture that fails validation is marked for a new study, including failures found after approval.
The captured files and unrelated
project work remain available for inspection. An unchanged approved capture remains reusable;
cancellation keeps its existing behavior. Unsigned website studies retain their existing flow.

The check inspects the root page and every captured route, including unindexed or incomplete
route folders. An unsigned first page cannot bypass confirmation for a signed-in route. Every
signed-in page must match the target website and current saved-account revision; mixed accounts,
missing authentication records, and unsafe route paths require a new study. Only signed-in pages
contribute preview screenshots, so a public landing page never stands in for private content.
The desktop and CLI use the same project-relative screenshot list. Changes to any nested capture
while approval is pending invalidate the answer.

A new study chooses its browser profile at the first open and keeps that authentication identity
through its route captures and Deep Mode. Saving a sign-in afterward does not upgrade an unsigned
study. A signed-in study refuses a different saved-account revision on reopen. Each Deep reader
(JavaScript analysis, observation and traffic recording) uses the verified browser's explicit
local endpoint, so it cannot fall back to another session or launch a replacement browser.
Root and Deep reopen failures stop the study before further readers or publication. An ordinary
optional route page-load failure is reported and skipped without photographing the failed page.
Every route that opens successfully still has its site, profile and account revision verified;
authentication integrity failures and cancellation stop immediately.

Optional route captures use a private staging folder in the project investigation storage.
Only successful captures are published under `capture/routes/`. A failed attempt is discarded
without removing successful routes or unrelated capture files. Cleanup failures stop the study;
unexpected existing folders or links remain untouched and fail closed.

Deep Mode records its profile provenance in `capture.json` after collecting its artifacts. The
approval gate checks that provenance against the captured pages before it can skip confirmation
for unsigned content. Older Deep captures without this record require a fresh study, even when
their screenshots were unsigned: the account used for their analysis cannot be established.
Valid signed-in Deep captures still require explicit screenshot confirmation. Rejected or
invalidated captures require another study before reuse.

The absence of a password field is not proof of authentication. A detected password page is
refused; the person must still recognize the screenshots. Old captures without a recorded
authentication state, and signed-in captures without an account revision, require a new study. Files are bounded to 32 MiB each, 512 MiB in total, and 2,048
entries; links and nonregular files are refused. These checks protect normal project storage;
they do not make an already trusted builder's shell a security sandbox.

This feature is independent of saved-sign-in management, browser attachment, sign-in refresh,
and website export limitations. No change from PR #2148 is required.
