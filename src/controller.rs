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
use anyhow::Result;

use operator_framework::install::container::*;

use crate::crd::Minecraft;
use k8s_openapi::api::apps::v1::{Deployment, DeploymentStrategy};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, OwnerReference};

use kube::api::{Meta, ObjectMeta, PostParams};
use kube::{Api, Client};

use std::collections::BTreeMap;
use std::fmt::Display;

use operator_framework::install::container::ApplyContainer;
use operator_framework::install::container::ApplyPort;
use operator_framework::install::container::SetResources;

use k8s_openapi::api::core::v1::{
    ConfigMap, Container, EmptyDirVolumeSource, HTTPGetAction, PersistentVolumeClaim,
    PersistentVolumeClaimVolumeSource, Probe, ResourceRequirements, Secret, SecretVolumeSource,
    Service, ServiceAccount, ServicePort, TCPSocketAction, Volume, VolumeMount,
};
use k8s_openapi::api::rbac::v1::{Role, RoleBinding, Subject};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;

use openshift_openapi::api::route::v1::{Route, RoutePort};

use operator_framework::process::create_or_update;
use operator_framework::utils::UseOrCreate;

use operator_framework::install::meta::OwnedBy;

pub struct MinecraftController {
    client: Client,
    deployments: Api<Deployment>,
    secrets: Api<Secret>,
    configmaps: Api<ConfigMap>,
    service_accounts: Api<ServiceAccount>,
    roles: Api<Role>,
    role_bindings: Api<RoleBinding>,
    services: Api<Service>,
    routes: Option<Api<Route>>,
    pvcs: Api<PersistentVolumeClaim>,
}

pub const KUBERNETES_LABEL_NAME: &str = "app.kubernetes.io/name";
pub const KUBERNETES_LABEL_INSTANCE: &str = "app.kubernetes.io/instance";
pub const KUBERNETES_LABEL_COMPONENT: &str = "app.kubernetes.io/component";

impl MinecraftController {
    pub fn new(namespace: &str, client: Client, has_openshift: bool) -> Self {
        MinecraftController {
            client: client.clone(),
            deployments: Api::namespaced(client.clone(), &namespace),
            secrets: Api::namespaced(client.clone(), &namespace),
            service_accounts: Api::namespaced(client.clone(), &namespace),
            roles: Api::namespaced(client.clone(), &namespace),
            role_bindings: Api::namespaced(client.clone(), &namespace),
            services: Api::namespaced(client.clone(), &namespace),
            configmaps: Api::namespaced(client.clone(), &namespace),
            pvcs: Api::namespaced(client.clone(), &namespace),
            routes: if has_openshift {
                Some(Api::namespaced(client.clone(), &namespace))
            } else {
                None
            },
        }
    }

    pub async fn reconcile(&self, original: &Minecraft) -> Result<()> {
        let minecraft = original.clone();

        let prefix = minecraft.name();
        let namespace = minecraft.namespace().expect("Missing namespace");

        log::info!("Reconcile: {}/{}", namespace, minecraft.name());

        let result = self.do_reconcile(minecraft).await;

        let minecraft = match result {
            Ok(mut minecraft) => {
                minecraft.status.use_or_create(|status| {
                    status.phase = "Active".into();
                    status.message = None;
                });
                minecraft
            }
            Err(err) => {
                let mut minecraft = original.clone();
                minecraft.status.use_or_create(|status| {
                    status.phase = "Failed".into();
                    status.message = Some(err.to_string());
                });
                minecraft
            }
        };

        if !original.eq(&minecraft) {
            Api::<Minecraft>::namespaced(self.client.clone(), &namespace)
                .replace_status(
                    &minecraft.name(),
                    &PostParams::default(),
                    serde_json::to_vec(&minecraft)?,
                )
                .await?;
        }

        Ok(())
    }

    fn resource_name<S>(&self, minecraft: &Minecraft, name: S) -> String
    where
        S: AsRef<str>,
    {
        format!("{}-{}", minecraft.name(), name.as_ref())
    }

    async fn do_reconcile(&self, minecraft: Minecraft) -> Result<Minecraft> {
        let namespace = minecraft.namespace();

        let service_account = self.resource_name(&minecraft, "server");
        let pvc = self.resource_name(&minecraft, "data");
        let service = self.resource_name(&minecraft, "minecraft");
        let tls_secret = self.resource_name(&minecraft, "minecraft-tls");

        create_or_update(
            &self.service_accounts,
            namespace.as_ref(),
            &service_account,
            |mut sa| {
                sa.owned_by_controller(&minecraft)?;
                Ok(sa)
            },
        )
        .await?;

        create_or_update(&self.pvcs, namespace.as_ref(), pvc.clone(), |mut pvc| {
            pvc.spec.use_or_create(|spec| {
                spec.access_modes = Some(vec!["ReadWriteOnce".into()]);
                spec.resources.use_or_create(|resources| {
                    resources.set_resources::<&str, &str, &str>("storage", Some("1Gi"), None);
                });
            });
            Ok(pvc)
        })
        .await?;

        self.deploy_server(
            &minecraft,
            namespace.as_ref(),
            service_account.clone(),
            tls_secret.clone(),
            pvc.clone(),
        )
        .await?;

        create_or_update(
            &self.services,
            namespace.as_ref(),
            service.clone(),
            |mut service| {
                service.metadata.annotations.use_or_create(|annotations| {
                    annotations.insert(
                        "service.beta.openshift.io/serving-cert-secret-name".into(),
                        tls_secret.clone(),
                    );
                });

                service.spec.use_or_create(|spec| {
                    spec.selector = Some(self.selector(&minecraft));
                    spec.type_ = Some("ClusterIP".into());
                    spec.ports = Some(vec![ServicePort {
                        name: Some("mc-tls".into()),
                        port: 11337,
                        protocol: Some("TCP".into()),
                        target_port: Some(IntOrString::String("mc-tls".into())),
                        ..Default::default()
                    }]);
                });
                Ok(service)
            },
        )
        .await?;

        if let Some(routes) = &self.routes {
            create_or_update(&routes, namespace.as_ref(), service.clone(), |mut route| {
                route.spec.tls.use_or_create(|tls| {
                    tls.termination = "Passthrough".into();
                    tls.insecure_edge_termination_policy = Some("None".into());
                });

                route.spec.port = Some(RoutePort {
                    target_port: IntOrString::String("mc-tls".into()),
                });

                route.spec.to.kind = "Service".into();
                route.spec.to.name = service.clone();
                route.spec.to.weight = 100;

                Ok(route)
            })
            .await?;
        }

        Ok(minecraft)
    }

    fn selector(&self, minecraft: &Minecraft) -> BTreeMap<String, String> {
        let prefix = minecraft.name();

        let mut selector = BTreeMap::new();
        selector.insert(KUBERNETES_LABEL_NAME.into(), "server".into());
        selector.insert(
            KUBERNETES_LABEL_INSTANCE.into(),
            format!("server-{}", &prefix),
        );
        selector.insert(KUBERNETES_LABEL_COMPONENT.into(), "server".into());

        selector
    }

    async fn deploy_server(
        &self,
        minecraft: &Minecraft,
        namespace: Option<&String>,
        service_account: String,
        tls_secret: String,
        pvc: String,
    ) -> Result<()> {
        create_or_update(
            &self.deployments,
            namespace,
            self.resource_name(&minecraft, "server"),
            |mut deployment| {
                deployment.spec.use_or_create_err(|spec| {
                    // always scale to 1
                    spec.replicas = Some(1);

                    let selector = self.selector(minecraft);

                    spec.selector = LabelSelector {
                        match_labels: Some(selector.clone()),
                        ..Default::default()
                    };

                    spec.strategy = Some(DeploymentStrategy {
                        type_: Some("Recreate".into()),
                        ..Default::default()
                    });

                    spec.template.metadata = Some(
                        ObjectMeta {
                            labels: Some(selector.clone()),
                            ..Default::default()
                        }
                    );

                    spec.template.spec.use_or_create_err(|pod_spec| {
                        pod_spec.service_account = Some(service_account);

                        // init container

                        pod_spec
                            .init_containers
                            .apply_container("download", |container| {
                                container.image = Some("registry.access.redhat.com/ubi8-minimal".into());
                                container.command = Some(vec![
                                    "bash".into(),
                                    "-c".into(),
                                    r#"
mkdir -p /data/server
test -f /data/server/server.jar || curl https://launcher.mojang.com/v1/objects/a412fd69db1f81db3f511c1463fd304675244077/server.jar -Ls -o /data/server/server.jar
echo "eula=true" > /data/eula.txt 
                                    "#
                                        .into(),
                                ]);
                                container.apply_volume_mount("data", |v| {
                                    v.mount_path = "/data".into();
                                    v.read_only = Some(false);
                                    Ok(())
                                })?;
                                Ok(())
                            })?;

                        // main container

                        pod_spec.containers.apply_container("server", |container| {
                            container.image = Some("docker.io/ctron/minecraft-base:latest".into());

                            container.resources.use_or_create(|res|{
                                res.set_resources("memory", Some("2Gi"), Some("2Gi"));
                            });

                            container.working_dir = Some("/data".into());
                            container.command = Some(vec![
                                "java",
                                "-XX:+PrintFlagsFinal",
                                "-jar",
                                "/data/server/server.jar",
                                "--nogui",
                                "--port",
                                "1337"].iter().map(|s|s.to_string()).collect()
                            );

                            container.apply_volume_mount("data", |v| {
                                v.mount_path = "/data".into();
                                v.read_only = Some(false);
                                Ok(())
                            })?;
                            container.apply_volume_mount("logs", |v| {
                                v.mount_path = "/logs".into();
                                v.read_only = Some(false);
                                Ok(())
                            })?;

                            container.readiness_probe = Some(Probe {
                                initial_delay_seconds: Some(30),
                                period_seconds: Some(10),
                                timeout_seconds: Some(1),
                                failure_threshold: Some(3),
                                tcp_socket: Some(TCPSocketAction{
                                    port: IntOrString::Int(1337),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });
                            container.liveness_probe = Some(Probe {
                                initial_delay_seconds: Some(30),
                                period_seconds: Some(10),
                                timeout_seconds: Some(3),
                                failure_threshold: Some(5),
                                tcp_socket: Some(TCPSocketAction{
                                    port: IntOrString::Int(1337),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });

                            Ok(())
                        })?;

                        pod_spec.containers.apply_container("tls", |container| {
                            container.image = Some("docker.io/ctron/minecraft-base:latest".into());

                            container.resources.use_or_create(|res|{
                                res.set_resources("memory", Some("64Mi"), Some("64Mi"));
                            });

                            container.command = Some(vec![
                                "stunnel".into(),
                                "/etc/mctunnel.conf".into()]
                            );

                            container.add_port("mc-tls", 11337, Some("TCP".into()))?;
                            container.apply_volume_mount("tls", |mount|{
                                mount.mount_path="/etc/mc-tls".into();
                                mount.read_only=Some(true);
                                Ok(())
                            })?;

                            container.readiness_probe = Some(Probe {
                                initial_delay_seconds: Some(30),
                                period_seconds: Some(10),
                                timeout_seconds: Some(1),
                                failure_threshold: Some(3),
                                tcp_socket: Some(TCPSocketAction{
                                    port: IntOrString::Int(11337),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });
                            container.liveness_probe = Some(Probe {
                                initial_delay_seconds: Some(30),
                                period_seconds: Some(10),
                                timeout_seconds: Some(3),
                                failure_threshold: Some(5),
                                tcp_socket: Some(TCPSocketAction{
                                    port: IntOrString::Int(11337),
                                    ..Default::default()
                                }),
                                ..Default::default()
                            });

                            Ok(())
                        })?;

                        // tls sidecar

                        pod_spec.apply_volume("data", |v| {
                            v.persistent_volume_claim = Some(PersistentVolumeClaimVolumeSource {
                                claim_name: pvc,
                                read_only: Some(false),
                            });
                            Ok(())
                        })?;
                        pod_spec.apply_volume("logs", |v| {
                            v.empty_dir = Some(EmptyDirVolumeSource {
                                ..Default::default()
                            });
                            Ok(())
                        })?;
                        pod_spec.apply_volume("tls", |v| {
                            v.secret=Some(SecretVolumeSource{
                                secret_name: Some(tls_secret),
                                ..Default::default()
                            });
                            Ok(())
                        })?;

                        Ok(())
                    })?;

                    Ok(())
                })?;

                Ok(deployment)
            },
        )
            .await?;

        Ok(())
    }
}
