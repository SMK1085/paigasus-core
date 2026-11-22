// SPDX-License-Identifier: Apache-2.0
// The policy texts of the tenant guide section "Detector findings" (SMA-855 spec section 6.12).
// `paigasus-iam-core` tests/detector_policies.rs validates each one against the IAM tool schema
// and checks that the guide holds each one verbatim. The gateway end-to-end tests read them too.
// One policy per paragraph. Each paragraph starts with its `@id`.

@id("detectors-block-high")
forbid (principal, action, resource)
when { context.max_severity >= 3 };

@id("detectors-block-kinds")
forbid (principal, action, resource)
when { context.findings.containsAny(["secret_in_args", "injection_markers", "name_collision"]) };

@id("fetch-hosts")
forbid (principal, action == Pgs::Gateway::Action::"fetch", resource)
unless { ["docs.example.com", "api.example.com"].containsAll(context.call.hosts) };

@id("fetch-deny-hosts")
forbid (principal, action == Pgs::Gateway::Action::"fetch", resource)
when { context.call.hosts.containsAny(["evil.example"]) || context.findings.contains("detector_error") };
