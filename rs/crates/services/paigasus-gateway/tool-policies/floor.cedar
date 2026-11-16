// SPDX-License-Identifier: Apache-2.0
@id("floor-name-collision") forbid(principal, action, resource) when { context.findings.contains("name_collision") };
