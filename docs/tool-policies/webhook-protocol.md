# Webhook provider protocol, version 1 (SMA-856)

This document is for the author of a webhook provider. A provider is a service that the platform
operator registers with the AI Gateway. The gateway sends it one signed request for each tool call.
The provider answers with findings. The gateway checks the signature of the answer.

A test in the gateway reads this file. It checks that the worked example at the end of this
document matches the code. Do not change a value of the example without a change of the test.

## The request

The gateway sends `POST <url>` with these headers:

- `Content-Type: application/json`
- `Accept-Encoding: identity`
- `Paigasus-Webhook-Version: 1`
- `Paigasus-Nonce: <32 lower-case hex characters>`. These are 16 random bytes. The nonce is new for
  each request.
- `Paigasus-Signature: t=<unix seconds>,v1=<64 lower-case hex characters>`

The `v1` value is this HMAC:

```text
v1 = HMAC-SHA256(secret, "paigasus.request." || name || "." || t || "." || nonce || "." || body)
```

- `name` is the provider name in the gateway configuration.
- `t` is the number in the header, in decimal digits.
- `nonce` is the text of the nonce header.
- `body` is the exact bytes of the request body.

### The key bytes

The key is the exact bytes of the secret. Do not decode the secret. Do not trim it. Do not treat it
as hex or base64. The secret has at least 32 bytes, with no leading or trailing whitespace.

### The request body

A provider must not rely on the order of the fields.

```json
{
  "version": 1,
  "request_id": "req_...",
  "subject": "prn:pgs:iam:::principal/...",
  "org": "6f1c0000-0000-4000-8000-000000000001",
  "session": "9a2e0000-0000-4000-8000-000000000002",
  "tool": { "name": "send_email", "declared": "yes" },
  "args": { "to": "x@y.example", "body": "..." },
  "args_text": null,
  "findings": []
}
```

- `request_id`, `org` and `session` are `null` when they are absent.
- `subject` is the PRN of the caller. `org` is the organization UUID.
- `tool.declared` is `yes`, `no` or `unknown`. It tells if the client offered the tool.
- `findings` holds the built-in findings of this call. A finding has `detector`, `kind`, `severity`,
  `version` and `evidence`. An evidence entry has `rule`, `location` (`argument` or `text_input`)
  and, when present, `pointer` and `host`.
- The gateway sends the arguments as the model sent them. It does not redact them.

### `args` and `args_text`

Exactly one of `args` and `args_text` is not `null`.

- When the argument text is valid JSON and has no duplicate key and no excess depth, `args` holds
  that JSON. The text goes in as it is, but the whitespace before and after the text is removed.
  The gateway does not write the JSON again. A number with many digits stays as the model wrote it.
- In every other case, `args` is `null` and `args_text` holds the argument text as a string. This
  includes text input (the tool kinds `custom`, `apply_patch` and `other`), invalid JSON and a JSON
  object with a duplicate key.

## The response

The provider must answer with status `200` and this header:

```text
Paigasus-Signature: v1=<64 lower-case hex characters>[,v1=<64 lower-case hex characters>]
```

The `v1` value is this HMAC:

```text
v1 = HMAC-SHA256(secret, "paigasus.response." || name || "." || nonce || "." || body)
```

`nonce` is the nonce of the request. `body` is the exact bytes of the response body. The header can
hold one or two `v1` values. The gateway accepts the answer when one value verifies with the
current secret or with the previous secret.

The body is JSON:

```json
{ "findings": [ { "kind": "pii_email", "severity": "medium", "version": "2026.11" } ] }
```

- `findings` is required. An empty list is a valid answer. It means no findings.
- A finding has `kind`, `severity` and, optionally, `version`. An absent `version` means `""`.
- `severity` is `info`, `low`, `medium` or `high`.
- `kind` must be one of the `kinds` that the operator registered for your provider.
- `version` has at most 32 characters from `a-z`, `0-9`, `.`, `_` and `-`.
- The answer has at most 16 findings, and the body has at most 64 KiB.
- Any other field, in the answer or in a finding, makes the answer malformed. Do not send a
  `detector` field.

## The order of checks

The gateway checks the answer in this order. Each failure gives a finding of the kind
`detector_error` for that call, and the reason is the `version` of the finding.

1. The status is `200`. Any other status is `status`. A redirect is `status`. The gateway follows
   no redirect.
2. `Content-Encoding` is absent or `identity`. Else the reason is `malformed`.
3. The body is read as a stream. A body over 64 KiB is `response_too_large`.
4. There is exactly one `Paigasus-Signature` header, and its form is correct. Else `signature`.
5. One `v1` value verifies in constant time with the current or the previous secret. Else
   `signature`.
6. `Content-Type` is `application/json`. Parameters, such as a charset, are allowed. Else
   `malformed`.
7. The JSON parse and the checks of the fields. Else `malformed`.

The gateway does not parse the body before it verifies the signature.

Other reasons come from the transport: `request_too_large`, `overloaded`, `transport`, `timeout`
and `deadline`.

## Replay protection

The gateway sends no request twice. The provider is responsible for the replay window. Reject a
request when `t` differs from your clock by more than 5 minutes. Do not accept a nonce twice in
that window.

## Rotation of the secret

Do these steps in this order:

1. The provider accepts requests that are signed with the old secret or with the new secret.
2. The operator sets `secret_env` to the new secret and `previous_secret_env` to the old secret.
   Then the operator rolls out the gateway.
3. The provider signs its responses with the new secret.
4. The operator removes `previous_secret_env`.

## A worked example

These values are fixed. Use them to test your code.

- Secret (the exact bytes, 36 characters): `paigasus-webhook-example-secret-0001`
- Name: `acme_dlp`
- `t`: `1764300000`
- Nonce: `00112233445566778899aabbccddeeff`

The request body (one line, with no trailing newline):

```text
{"version":1,"request_id":"req_example","subject":"prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004","org":"6f1c0000-0000-4000-8000-000000000001","session":"9a2e0000-0000-4000-8000-000000000002","tool":{"name":"send_email","declared":"yes"},"args":{"to":"x@y.example","body":"hi"},"args_text":null,"findings":[]}
```

The request MAC:

```text
4929f346b96d703a5ab953c6e8981444573fa64e66795b8a5d61ba680c5ba878
```

So the request header is:

```text
Paigasus-Signature: t=1764300000,v1=4929f346b96d703a5ab953c6e8981444573fa64e66795b8a5d61ba680c5ba878
```

The answer body:

```text
{"findings":[{"kind":"pii_email","severity":"medium","version":"2026.11"}]}
```

The response MAC:

```text
cf6147bdf8be4b7c7b4619accf9b1f592a14d6d9209e112a1a86c7e00d6f5638
```

So the response header is:

```text
Paigasus-Signature: v1=cf6147bdf8be4b7c7b4619accf9b1f592a14d6d9209e112a1a86c7e00d6f5638
```
