use yew::prelude::*;
use serde::{Deserialize, Serialize};
use web_sys::HtmlInputElement;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct User {
    pub id: String,
    pub username: String,
    pub email: String,
    pub role: String,
    pub realm: String,
    pub enabled: bool,
    pub created: String,
    pub last_login: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Role {
    pub name: String,
    pub permissions: Vec<String>,
    pub description: String,
}

#[derive(Properties, PartialEq)]
pub struct Props {}

pub struct UserManagementPage {
    users: Vec<User>,
    roles: Vec<Role>,
    loading: bool,
    show_create_user: bool,
    show_create_role: bool,
    // User form
    new_username: String,
    new_email: String,
    new_password: String,
    new_role: String,
    new_realm: String,
    // Role form
    new_role_name: String,
    new_role_desc: String,
    new_role_perms: Vec<String>,
}

pub enum Msg {
    LoadUsers,
    LoadRoles,
    UsersLoaded(Vec<User>),
    RolesLoaded(Vec<Role>),
    ToggleCreateUser,
    ToggleCreateRole,
    UpdateUsername(String),
    UpdateEmail(String),
    UpdatePassword(String),
    UpdateUserRole(String),
    UpdateRealm(String),
    CreateUser,
    UserCreated,
    UpdateRoleName(String),
    UpdateRoleDesc(String),
    TogglePermission(String),
    CreateRole,
    RoleCreated,
    ToggleUser(String),
    DeleteUser(String),
    DeleteRole(String),
    #[allow(dead_code)]
    Error(String),
}

impl Component for UserManagementPage {
    type Message = Msg;
    type Properties = Props;

    fn create(ctx: &Context<Self>) -> Self {
        ctx.link().send_message(Msg::LoadUsers);
        ctx.link().send_message(Msg::LoadRoles);

        Self {
            users: Vec::new(),
            roles: Vec::new(),
            loading: true,
            show_create_user: false,
            show_create_role: false,
            new_username: String::new(),
            new_email: String::new(),
            new_password: String::new(),
            new_role: "user".to_string(),
            new_realm: "local".to_string(),
            new_role_name: String::new(),
            new_role_desc: String::new(),
            new_role_perms: Vec::new(),
        }
    }

    fn update(&mut self, ctx: &Context<Self>, msg: Self::Message) -> bool {
        match msg {
            Msg::LoadUsers => {
                self.loading = true;
                let link = ctx.link().clone();

                wasm_bindgen_futures::spawn_local(async move {
                    let users = vec![
                        User {
                            id: "user1".to_string(),
                            username: "admin".to_string(),
                            email: "admin@horcrux.local".to_string(),
                            role: "administrator".to_string(),
                            realm: "local".to_string(),
                            enabled: true,
                            created: "2025-01-01".to_string(),
                            last_login: Some("2025-10-08 10:30".to_string()),
                        },
                        User {
                            id: "user2".to_string(),
                            username: "operator".to_string(),
                            email: "ops@horcrux.local".to_string(),
                            role: "operator".to_string(),
                            realm: "local".to_string(),
                            enabled: true,
                            created: "2025-02-15".to_string(),
                            last_login: Some("2025-10-07 14:22".to_string()),
                        },
                    ];

                    link.send_message(Msg::UsersLoaded(users));
                });

                true
            }

            Msg::LoadRoles => {
                let link = ctx.link().clone();

                wasm_bindgen_futures::spawn_local(async move {
                    let roles = vec![
                        Role {
                            name: "administrator".to_string(),
                            permissions: vec![
                                "VM.*".to_string(),
                                "Storage.*".to_string(),
                                "User.*".to_string(),
                                "Cluster.*".to_string(),
                            ],
                            description: "Full system access".to_string(),
                        },
                        Role {
                            name: "operator".to_string(),
                            permissions: vec![
                                "VM.PowerMgmt".to_string(),
                                "VM.Console".to_string(),
                                "VM.Monitor".to_string(),
                            ],
                            description: "VM operations only".to_string(),
                        },
                        Role {
                            name: "user".to_string(),
                            permissions: vec![
                                "VM.Console".to_string(),
                                "VM.Monitor".to_string(),
                            ],
                            description: "Read-only VM access".to_string(),
                        },
                    ];

                    link.send_message(Msg::RolesLoaded(roles));
                });

                true
            }

            Msg::UsersLoaded(users) => {
                self.users = users;
                self.loading = false;
                true
            }

            Msg::RolesLoaded(roles) => {
                self.roles = roles;
                true
            }

            Msg::ToggleCreateUser => {
                self.show_create_user = !self.show_create_user;
                if !self.show_create_user {
                    self.new_username.clear();
                    self.new_email.clear();
                    self.new_password.clear();
                }
                true
            }

            Msg::ToggleCreateRole => {
                self.show_create_role = !self.show_create_role;
                if !self.show_create_role {
                    self.new_role_name.clear();
                    self.new_role_desc.clear();
                    self.new_role_perms.clear();
                }
                true
            }

            Msg::UpdateUsername(username) => {
                self.new_username = username;
                true
            }

            Msg::UpdateEmail(email) => {
                self.new_email = email;
                true
            }

            Msg::UpdatePassword(password) => {
                self.new_password = password;
                true
            }

            Msg::UpdateUserRole(role) => {
                self.new_role = role;
                true
            }

            Msg::UpdateRealm(realm) => {
                self.new_realm = realm;
                true
            }

            Msg::CreateUser => {
                let link = ctx.link().clone();

                wasm_bindgen_futures::spawn_local(async move {
                    web_sys::console::log_1(&"Creating user...".into());
                    link.send_message(Msg::UserCreated);
                });

                true
            }

            Msg::UserCreated => {
                self.show_create_user = false;
                ctx.link().send_message(Msg::LoadUsers);
                true
            }

            Msg::UpdateRoleName(name) => {
                self.new_role_name = name;
                true
            }

            Msg::UpdateRoleDesc(desc) => {
                self.new_role_desc = desc;
                true
            }

            Msg::TogglePermission(perm) => {
                if self.new_role_perms.contains(&perm) {
                    self.new_role_perms.retain(|p| p != &perm);
                } else {
                    self.new_role_perms.push(perm);
                }
                true
            }

            Msg::CreateRole => {
                let link = ctx.link().clone();

                wasm_bindgen_futures::spawn_local(async move {
                    web_sys::console::log_1(&"Creating role...".into());
                    link.send_message(Msg::RoleCreated);
                });

                true
            }

            Msg::RoleCreated => {
                self.show_create_role = false;
                ctx.link().send_message(Msg::LoadRoles);
                true
            }

            Msg::ToggleUser(user_id) => {
                if let Some(user) = self.users.iter_mut().find(|u| u.id == user_id) {
                    user.enabled = !user.enabled;
                }
                true
            }

            Msg::DeleteUser(user_id) => {
                self.users.retain(|u| u.id != user_id);
                true
            }

            Msg::DeleteRole(role_name) => {
                self.roles.retain(|r| r.name != role_name);
                true
            }

            Msg::Error(err) => {
                web_sys::console::error_1(&err.into());
                false
            }
        }
    }

    fn view(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div class="container">
                <div class="header">
                    <h1>{ "User Management" }</h1>
                </div>

                <div class="tabs">
                    <button class="tab active">{ "Users" }</button>
                    <button class="tab">{ "Roles & Permissions" }</button>
                </div>

                // Users section
                <div class="section">
                    <div class="section-header">
                        <h2>{ "Users" }</h2>
                        <button
                            class="btn-primary"
                            onclick={ctx.link().callback(|_| Msg::ToggleCreateUser)}
                        >
                            { if self.show_create_user { "Cancel" } else { "+ New User" } }
                        </button>
                    </div>

                    { self.view_create_user_form(ctx) }

                    { if self.loading {
                        html! { <div class="loading">{ "Loading users..." }</div> }
                    } else {
                        self.view_users_table(ctx)
                    }}
                </div>

                // Roles section
                <div class="section">
                    <div class="section-header">
                        <h2>{ "Roles" }</h2>
                        <button
                            class="btn-primary"
                            onclick={ctx.link().callback(|_| Msg::ToggleCreateRole)}
                        >
                            { if self.show_create_role { "Cancel" } else { "+ New Role" } }
                        </button>
                    </div>

                    { self.view_create_role_form(ctx) }
                    { self.view_roles_list(ctx) }
                </div>
            </div>
        }
    }
}

#[allow(dead_code)]
impl UserManagementPage {
    fn view_create_user_form(&self, ctx: &Context<Self>) -> Html {
        if !self.show_create_user {
            return html! {};
        }

        html! {
            <div class="create-form card">
                <h3>{ "Create New User" }</h3>

                <div class="form-group">
                    <label>{ "Username" }</label>
                    <input
                        type="text"
                        value={self.new_username.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateUsername(input.value())
                        })}
                        placeholder="username"
                    />
                </div>

                <div class="form-group">
                    <label>{ "Email" }</label>
                    <input
                        type="email"
                        value={self.new_email.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateEmail(input.value())
                        })}
                        placeholder="user@example.com"
                    />
                </div>

                <div class="form-group">
                    <label>{ "Password" }</label>
                    <input
                        type="password"
                        value={self.new_password.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdatePassword(input.value())
                        })}
                        placeholder="••••••••"
                    />
                </div>

                <div class="form-row">
                    <div class="form-group">
                        <label>{ "Role" }</label>
                        <select
                            value={self.new_role.clone()}
                            onchange={ctx.link().callback(|e: Event| {
                                let select: HtmlInputElement = e.target_unchecked_into();
                                Msg::UpdateUserRole(select.value())
                            })}
                        >
                            { for self.roles.iter().map(|r| html! {
                                <option value={r.name.clone()}>{ &r.name }</option>
                            })}
                        </select>
                    </div>

                    <div class="form-group">
                        <label>{ "Realm" }</label>
                        <select
                            value={self.new_realm.clone()}
                            onchange={ctx.link().callback(|e: Event| {
                                let select: HtmlInputElement = e.target_unchecked_into();
                                Msg::UpdateRealm(select.value())
                            })}
                        >
                            <option value="local">{ "Local" }</option>
                            <option value="ldap">{ "LDAP" }</option>
                            <option value="oidc">{ "OIDC" }</option>
                        </select>
                    </div>
                </div>

                <div class="form-actions">
                    <button
                        class="btn-primary"
                        onclick={ctx.link().callback(|_| Msg::CreateUser)}
                        disabled={self.new_username.is_empty() || self.new_password.is_empty()}
                    >
                        { "Create User" }
                    </button>
                    <button
                        class="btn-secondary"
                        onclick={ctx.link().callback(|_| Msg::ToggleCreateUser)}
                    >
                        { "Cancel" }
                    </button>
                </div>
            </div>
        }
    }

    fn view_users_table(&self, ctx: &Context<Self>) -> Html {
        html! {
            <table class="data-table">
                <thead>
                    <tr>
                        <th>{ "Username" }</th>
                        <th>{ "Email" }</th>
                        <th>{ "Role" }</th>
                        <th>{ "Realm" }</th>
                        <th>{ "Last Login" }</th>
                        <th>{ "Status" }</th>
                        <th>{ "Actions" }</th>
                    </tr>
                </thead>
                <tbody>
                    { for self.users.iter().map(|user| self.view_user_row(ctx, user)) }
                </tbody>
            </table>
        }
    }

    fn view_user_row(&self, ctx: &Context<Self>, user: &User) -> Html {
        let user_id = user.id.clone();
        let delete_id = user.id.clone();

        html! {
            <tr>
                <td>{ &user.username }</td>
                <td>{ &user.email }</td>
                <td><span class="badge">{ &user.role }</span></td>
                <td>{ &user.realm }</td>
                <td>
                    { if let Some(ref login) = user.last_login {
                        html! { <span>{ login }</span> }
                    } else {
                        html! { <span class="muted">{ "Never" }</span> }
                    }}
                </td>
                <td>
                    <span class={classes!("status-badge", if user.enabled { "active" } else { "inactive" })}>
                        { if user.enabled { "Enabled" } else { "Disabled" } }
                    </span>
                </td>
                <td class="actions">
                    <button
                        class="btn-icon"
                        onclick={ctx.link().callback(move |_| Msg::ToggleUser(user_id.clone()))}
                        title={if user.enabled { "Disable" } else { "Enable" }}
                    >
                        { if user.enabled { "⏸" } else { "▶" } }
                    </button>
                    <button
                        class="btn-icon btn-danger"
                        onclick={ctx.link().callback(move |_| Msg::DeleteUser(delete_id.clone()))}
                        title="Delete"
                    >
                        { "🗑" }
                    </button>
                </td>
            </tr>
        }
    }

    fn view_create_role_form(&self, ctx: &Context<Self>) -> Html {
        if !self.show_create_role {
            return html! {};
        }

        let available_perms = vec![
            "VM.Allocate", "VM.PowerMgmt", "VM.Config.*", "VM.Console",
            "VM.Backup", "VM.Snapshot", "VM.Migrate", "VM.Monitor",
            "Storage.*", "Network.*", "User.*", "Cluster.*",
        ];

        html! {
            <div class="create-form card">
                <h3>{ "Create New Role" }</h3>

                <div class="form-group">
                    <label>{ "Role Name" }</label>
                    <input
                        type="text"
                        value={self.new_role_name.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateRoleName(input.value())
                        })}
                    />
                </div>

                <div class="form-group">
                    <label>{ "Description" }</label>
                    <input
                        type="text"
                        value={self.new_role_desc.clone()}
                        oninput={ctx.link().callback(|e: InputEvent| {
                            let input: HtmlInputElement = e.target_unchecked_into();
                            Msg::UpdateRoleDesc(input.value())
                        })}
                    />
                </div>

                <div class="form-group">
                    <label>{ "Permissions" }</label>
                    <div class="permissions-grid">
                        { for available_perms.iter().map(|perm| {
                            let perm_str = perm.to_string();
                            let is_checked = self.new_role_perms.contains(&perm_str);

                            html! {
                                <label class="checkbox-label">
                                    <input
                                        type="checkbox"
                                        checked={is_checked}
                                        onchange={ctx.link().callback(move |_| {
                                            Msg::TogglePermission(perm_str.clone())
                                        })}
                                    />
                                    <span>{ perm }</span>
                                </label>
                            }
                        })}
                    </div>
                </div>

                <div class="form-actions">
                    <button
                        class="btn-primary"
                        onclick={ctx.link().callback(|_| Msg::CreateRole)}
                        disabled={self.new_role_name.is_empty()}
                    >
                        { "Create Role" }
                    </button>
                    <button
                        class="btn-secondary"
                        onclick={ctx.link().callback(|_| Msg::ToggleCreateRole)}
                    >
                        { "Cancel" }
                    </button>
                </div>
            </div>
        }
    }

    fn view_roles_list(&self, ctx: &Context<Self>) -> Html {
        html! {
            <div class="roles-grid">
                { for self.roles.iter().map(|role| self.view_role_card(ctx, role)) }
            </div>
        }
    }

    fn view_role_card(&self, ctx: &Context<Self>, role: &Role) -> Html {
        let role_name = role.name.clone();

        html! {
            <div class="role-card card">
                <div class="role-header">
                    <h4>{ &role.name }</h4>
                    <button
                        class="btn-icon btn-danger"
                        onclick={ctx.link().callback(move |_| Msg::DeleteRole(role_name.clone()))}
                        title="Delete"
                    >
                        { "🗑" }
                    </button>
                </div>

                <p class="role-description">{ &role.description }</p>

                <div class="permissions-list">
                    <strong>{ "Permissions:" }</strong>
                    <ul>
                        { for role.permissions.iter().map(|perm| html! {
                            <li><code>{ perm }</code></li>
                        })}
                    </ul>
                </div>
            </div>
        }
    }
}
