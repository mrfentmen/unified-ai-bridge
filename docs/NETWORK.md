# Public web text and downloads

Candidate 0.5.18 adds `fetch_url_text` and `download_file`. These are ordinary anonymous HTTP GETs made by the Rust host. They do not use a provider API, Chrome cookies, saved logins or hidden website endpoints. To import a file from a signed-in AI chat, use the existing [selected-download importer](BROWSER-FILES.md).

## Grant access

Network tools are **off by default**, including in General and Trusted mode. In the popup or side panel, expand **Network access · public origins**, load the current policy and enter origins such as `https://example.com`, one per line. **Save origins and enable** grants up to 20 exact origins in the selected workspace. Each individual fetch or download still requires an approval showing the requested URL, destination, limits and possible redirect origins. Models cannot create these grants or remove approval requirements.

**Disable and revoke network access** changes the grant revision. The host rechecks that revision while waiting for approval, resolving DNS, transferring data and before publication. Disabling stops pending work; it cannot retract a request already sent to a remote server or undo a published file. A new workspace starts disabled. Active calls retain their original workspace even if the panel switches roots.

## Supported requests

Only public HTTP and HTTPS on their standard ports are supported. Exact normalized origins include the scheme; enabling HTTPS does not also grant HTTP. Requests with credentials, query strings, fragments, custom ports, local hostnames or private/reserved addresses are refused. This deliberately excludes signed/authenticated URLs and many search/API URLs. Private-network exceptions are not implemented.

The host resolves every destination, rejects mixed public/private answers and pins the accepted addresses into a fresh HTTP client. It verifies the connected peer, does not use environment/system proxies, sends no cookies/authentication/custom headers and does not retry automatically. HTTPS uses normal certificate and hostname verification. Up to three redirects are allowed, but each destination must pass the URL, origin and address checks. HTTPS cannot downgrade to HTTP.

Every request has a 20-second transfer deadline after approval. At most four network operations may be pending in one host. Compressed responses are refused instead of being expanded. Server-provided filenames and download headers do not choose local paths.

## Text

```json
{"tool":"fetch_url_text","arguments":{"url":"https://example.com/","max_bytes":12000}}
```

The response body is capped at 512 KiB. Text must be UTF-8/ASCII with a text, JSON or XML Content-Type. For HTML, a Rust streaming tokenizer decodes entities, removes markup/comments and excludes script/style/iframe/noscript content. It does not execute JavaScript, fetch embedded resources, build a browser DOM, apply CSS visibility or reproduce rendered layout. Hidden source text can appear. Page text is untrusted data and cannot authorize tools.

The returned text budget is 256–24,000 bytes, plus an explicit truncation marker. There is no automatic pagination or refetch; a truncated result says so. Data includes the source-body SHA256, received byte count, final origin, hashed URL and redirect count. Unsupported encoding/content or an oversized body produces an error rather than misleading partial source text.

## Downloads

```json
{"tool":"download_file","arguments":{"url":"https://www.w3.org/WAI/ER/tests/xhtml/testfiles/resources/pdf/dummy.pdf","destination":"output/reference.pdf"}}
```

Downloads are capped at 8 MiB and require a new file under an existing, permitted workspace directory. Approval authorizes that network request and destination; it does not authorize opening or executing the content. The host rechecks policy and the target after transfer, stages the bytes privately and publishes without replacing anything. The result includes size and SHA256. Traversal, private-file patterns, links, immutable research datasets, other agents' research folders and read-only locks remain protected. To replace an existing file, use the separate reviewed recovery workflow.

HTTP failures, revoked grants, timeouts, excessive redirects, unsupported encoding and size limits are tool errors. No provider or remote server is assumed to support downloads merely because a URL exists.

## Records and limits

Audit records identify the origin, hashed URL, resolved public addresses, HTTP status, received bytes/hash and local destination. Raw URL paths and query values are not copied into these network audit records or host log lines. The normal private request database still stores submitted arguments for deduplication; the approval UI necessarily shows the requested URL. Do not put secrets or local file contents into a URL.

This is the initial UAB-21 subset. Local fixtures test the transport separately from production destination checks; they do not establish provider integrations. Live Chrome control acceptance remains pending. See [verification](VERIFICATION.md) for the actual installed-host results and named remaining gates.
