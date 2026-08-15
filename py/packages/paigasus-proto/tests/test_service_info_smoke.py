# SPDX-License-Identifier: Apache-2.0
from paigasus_proto.generated.paigasus.common.v1 import Capability, ServiceInfo


def test_service_info_carries_a_capability_list() -> None:
    info = ServiceInfo(service="iam", version="1.4.0", capabilities=["iam.audit"])
    assert info.service == "iam"
    assert info.version == "1.4.0"
    assert info.capabilities == ["iam.audit"]


def test_service_info_defaults_to_no_capabilities() -> None:
    # "absent capability -> feature off" starts from an empty list, not None.
    assert ServiceInfo().capabilities == []


def test_capability_registry_keeps_the_proto_names() -> None:
    names = Capability.betterproto_value_to_renamed_proto_names()
    assert names[Capability.IAM_AUTHZ_CEDAR.value] == "CAPABILITY_IAM_AUTHZ_CEDAR"
    assert names[Capability.IAM_APIKEYS.value] == "CAPABILITY_IAM_APIKEYS"
    assert names[Capability.IAM_AUDIT.value] == "CAPABILITY_IAM_AUDIT"
    assert names[Capability.GATEWAY_CHAT_STREAM.value] == "CAPABILITY_GATEWAY_CHAT_STREAM"
