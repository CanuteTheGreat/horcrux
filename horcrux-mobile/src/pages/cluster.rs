//! Cluster pages for mobile UI

use yew::prelude::*;
use wasm_bindgen_futures::spawn_local;

use crate::api::{ApiClient, ClusterStatus, NodeInfo};
use crate::components::{Header, Card, Loading};

#[function_component(ClusterView)]
pub fn cluster_view() -> Html {
    let cluster = use_state(|| None::<ClusterStatus>);
    let loading = use_state(|| true);

    {
        let cluster = cluster.clone();
        let loading = loading.clone();

        use_effect_with((), move |_| {
            spawn_local(async move {
                match ApiClient::get_cluster_status().await {
                    Ok(status) => cluster.set(Some(status)),
                    Err(_) => {}
                }
                loading.set(false);
            });

            || ()
        });
    }

    if *loading {
        return html! { <Loading /> };
    }

    html! {
        <div class="cluster-page">
            <Header title="Cluster" />

            <div class="page-content">
                {if let Some(ref cluster_status) = *cluster {
                    html! {
                        <>
                            <Card title="Cluster Information">
                                <div class="detail-grid">
                                    <div class="detail-row">
                                        <span class="label">{"Name:"}</span>
                                        <span class="value">{&cluster_status.name}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Nodes:"}</span>
                                        <span class="value">{cluster_status.nodes.len()}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Quorum:"}</span>
                                        <span class={if cluster_status.quorum { "value success" } else { "value error" }}>
                                            {if cluster_status.quorum { "Yes" } else { "No" }}
                                        </span>
                                    </div>
                                </div>
                            </Card>

                            <Card title="Nodes">
                                {cluster_status.nodes.iter().map(|node| {
                                    html! { <NodeCard node={node.clone()} /> }
                                }).collect::<Html>()}
                            </Card>
                        </>
                    }
                } else {
                    html! {
                        <div class="empty-state">
                            <p>{"No cluster information available"}</p>
                        </div>
                    }
                }}
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
struct NodeCardProps {
    node: NodeInfo,
}

#[function_component(NodeCard)]
fn node_card(props: &NodeCardProps) -> Html {
    let node = &props.node;

    html! {
        <div class="node-item">
            <div class="node-header">
                <span class="node-name">{&node.name}</span>
                <span class={if node.online { "status-badge status-running" } else { "status-badge status-stopped" }}>
                    {if node.online { "Online" } else { "Offline" }}
                </span>
            </div>
            <div class="node-details">
                <span class="node-detail">{"ID: "}{&node.id}</span>
                <span class="node-detail">{"Arch: "}{&node.architecture}</span>
                {if node.local {
                    html! { <span class="node-badge">{"Local"}</span> }
                } else {
                    html! {}
                }}
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct NodeDetailProps {
    pub node_id: String,
}

#[function_component(NodeDetail)]
pub fn node_detail(_props: &NodeDetailProps) -> Html {
    html! {
        <div class="node-detail-page">
            <Header title="Node Details" show_back={true} />
            <div class="page-content">
                <p>{"Node detail page - Coming soon"}</p>
            </div>
        </div>
    }
}
