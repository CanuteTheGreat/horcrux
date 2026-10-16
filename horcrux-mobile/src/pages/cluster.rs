//! Cluster pages for mobile UI

use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::{ApiClient, ClusterStatus, NodeInfo};
use crate::components::{Card, Header, Loading};

#[function_component(ClusterView)]
pub fn cluster_view() -> Html {
    let cluster = use_state(|| None::<ClusterStatus>);
    let nodes = use_state(Vec::<NodeInfo>::new);
    let loading = use_state(|| true);

    {
        let cluster = cluster.clone();
        let nodes = nodes.clone();
        let loading = loading.clone();

        use_effect_with((), move |_| {
            spawn_local(async move {
                if let Ok(status) = ApiClient::get_cluster_status().await {
                    cluster.set(Some(status))
                }
                if let Ok(list) = ApiClient::list_cluster_nodes().await {
                    nodes.set(list)
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
                                        <span class="value">{cluster_status.display_name()}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Nodes:"}</span>
                                        <span class="value">{format!("{} / {} online", cluster_status.online_nodes, cluster_status.total_nodes)}</span>
                                    </div>
                                    <div class="detail-row">
                                        <span class="label">{"Quorum:"}</span>
                                        <span class={cluster_status.quorum_display().1}>
                                            {cluster_status.quorum_display().0}
                                        </span>
                                    </div>
                                </div>
                            </Card>

                            <Card title="Nodes">
                                {nodes.iter().map(|node| {
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
                <span class={if node.is_online() { "status-badge status-running" } else { "status-badge status-stopped" }}>
                    {node.status.clone()}
                </span>
            </div>
            <div class="node-details">
                <span class="node-detail">{"ID: "}{node.id}</span>
                <span class="node-detail">{"Arch: "}{&node.architecture}</span>
                <span class="node-detail">{node.usage_label()}</span>
                {if node.is_local {
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
