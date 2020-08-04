/**
 * Copyright (c) 2020 Jens Reimann.
 *
 * See the NOTICE file(s) distributed with this work for additional
 * information regarding copyright ownership.
 *
 * This program and the accompanying materials are made available under the
 * terms of the Eclipse Public License 2.0 which is available at
 * http://www.eclipse.org/legal/epl-2.0
 *
 * SPDX-License-Identifier: EPL-2.0
 */
use kube_derive::CustomResource;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Serialize, Deserialize, Default, Debug, Clone, PartialEq)]
#[kube(
group = "minecraft.dentrassi.de",
version = "v1alpha1",
kind = "Minecraft",
namespaced,
derive = "PartialEq",
status = "MinecraftStatus"
)]
#[kube(apiextensions = "v1")]
#[serde(default, rename_all = "camelCase")]
pub struct MinecraftSpec {
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default)]
pub struct MinecraftStatus {
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod test {

    use super::*;
    use k8s_openapi::Resource;

    #[test]
    fn verify_resource() {
        assert_eq!(Minecraft::KIND, "Minecraft");
        assert_eq!(Minecraft::GROUP, "minecraft.dentrassi.de");
        assert_eq!(Minecraft::VERSION, "v1alpha1");
        assert_eq!(Minecraft::API_VERSION, "minecraft.dentrassi.de/v1alpha1");
    }
}
