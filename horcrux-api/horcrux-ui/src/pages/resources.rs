//! Resources (`/resources`): one search over VMs, containers, storages and
//! pools (text, kind, tag, status) and the resource tree.
use crate::api::{self, InvResource, InvTagCount, InvTreeNode};
use leptos::*;

fn tree_view(n: InvTreeNode) -> View {
    let label = format!(
        "{} {}",
        n.name,
        if n.status.is_empty() {
            String::new()
        } else {
            format!("[{}]", n.status)
        }
    );
    let tags = n.tags.join(", ");
    let has_kids = !n.children.is_empty();
    let kids = n.children.into_iter().map(tree_view).collect_view();
    let link = n.link.clone();
    view! {
        <li class=format!("tree-node tree-{}", n.kind) data-kind=n.kind.clone() data-id=n.id.clone()>
            {if link.is_empty() || link == "/" {
                view! { <span class="tree-label">{label.clone()}</span> }.into_view()
            } else {
                view! { <a class="tree-label" href=link>{label.clone()}</a> }.into_view()
            }}
            {(!tags.is_empty()).then(|| view! { <small class="tree-tags">" ("{tags}")"</small> })}
            {has_kids.then(|| view! { <ul>{kids}</ul> })}
        </li>
    }
    .into_view()
}

#[component]
pub fn ResourcesPage() -> impl IntoView {
    let (text, set_text) = create_signal(String::new());
    let (kind, set_kind) = create_signal(String::new());
    let (tag, set_tag) = create_signal(String::new());
    let (status, set_status) = create_signal(String::new());
    let (results, set_results) = create_signal(Vec::<InvResource>::new());
    let (tags, set_tags) = create_signal(Vec::<InvTagCount>::new());
    let (tree, set_tree) = create_signal(None::<InvTreeNode>);
    let (error, set_error) = create_signal(None::<String>);

    let run = move || {
        let (q, k, t, s) = (
            text.get_untracked(),
            kind.get_untracked(),
            tag.get_untracked(),
            status.get_untracked(),
        );
        spawn_local(async move {
            match api::search_resources(&q, &k, &t, &s).await {
                Ok(r) => {
                    set_results.set(r);
                    set_error.set(None);
                }
                Err(e) => set_error.set(Some(e.message)),
            }
        });
    };
    create_effect(move |_| {
        run();
        spawn_local(async move {
            if let Ok(t) = api::list_inventory_tags().await {
                set_tags.set(t);
            }
            if let Ok(t) = api::get_resource_tree().await {
                set_tree.set(Some(t));
            }
        });
    });
    create_effect(move |_| {
        kind.track();
        tag.track();
        status.track();
        run();
    });

    view! {
        <div class="resources">
            <div class="page-header"><h1>"Resources"</h1></div>
            {move || error.get().map(|e| view! { <div class="alert alert-error" id="res-error">{e}</div> })}
            <div class="form-row">
                <input id="res-search" type="search" placeholder="Search name, id, tag, pool" style="flex: 3"
                    prop:value=text on:input=move |ev| set_text.set(event_target_value(&ev))
                    on:keydown=move |ev| if ev.key() == "Enter" { run() }/>
                <select id="res-kind" on:change=move |ev| set_kind.set(event_target_value(&ev))>
                    <option value="">"All types"</option>
                    <option value="vm">"VMs"</option>
                    <option value="container">"Containers"</option>
                    <option value="storage">"Storage"</option>
                    <option value="pool">"Pools"</option>
                </select>
                <select id="res-status" on:change=move |ev| set_status.set(event_target_value(&ev))>
                    <option value="">"Any state"</option>
                    <option value="running">"running"</option>
                    <option value="stopped">"stopped"</option>
                </select>
                <button id="res-go" class="btn btn-primary" on:click=move |_| run()>"Search"</button>
            </div>
            <div class="tag-filter" id="res-tags">
                <span>"Tags: "</span>
                <button class=move || if tag.get().is_empty() { "btn btn-sm btn-primary" } else { "btn btn-sm" }
                    on:click=move |_| set_tag.set(String::new())>"all"</button>
                {move || tags.get().into_iter().map(|t| {
                    let name = t.tag.clone();
                    let name2 = t.tag.clone();
                    view! {
                        <button class=move || if tag.get() == name { "btn btn-sm btn-primary res-tag" } else { "btn btn-sm res-tag" }
                            on:click=move |_| set_tag.set(name2.clone())>{format!("{} ({})", t.tag, t.count)}</button>
                    }
                }).collect_view()}
            </div>
            <table class="vm-table" id="res-table">
                <thead><tr><th>"Type"</th><th>"Name"</th><th>"ID"</th><th>"State"</th><th>"Pool"</th><th>"Tags"</th></tr></thead>
                <tbody>
                    {move || {
                        let r = results.get();
                        if r.is_empty() {
                            view! { <tr><td colspan="6" class="no-data">"Nothing matches."</td></tr> }.into_view()
                        } else {
                            r.into_iter().map(|x| view! {
                                <tr class="res-row" data-kind=x.kind.clone() data-id=x.id.clone()>
                                    <td>{x.kind.clone()}</td>
                                    <td><a href=x.link.clone()>{x.name.clone()}</a></td>
                                    <td>{x.id.clone()}</td>
                                    <td>{x.status.clone()}</td>
                                    <td>{x.pool.clone().unwrap_or_default()}</td>
                                    <td>{x.tags.join(", ")}</td>
                                </tr>
                            }).collect_view()
                        }
                    }}
                </tbody>
            </table>
            <h2>"Tree"</h2>
            <ul class="resource-tree" id="res-tree">
                {move || tree.get().map(tree_view)}
            </ul>
        </div>
    }
}
