use leptos::prelude::*;

use crate::api::{login, set_token};

#[component]
pub fn Login() -> impl IntoView {
    let username = RwSignal::new(String::new());
    let password = RwSignal::new(String::new());
    let error = RwSignal::new(Option::<String>::None);
    let loading = RwSignal::new(false);

    let submit = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let u = username.get();
        let p = password.get();
        loading.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match login(&u, &p).await {
                Ok(token) => {
                    set_token(&token);
                    let _ = web_sys::window()
                        .and_then(|w| w.location().assign("/").ok());
                }
                Err(msg) => {
                    error.set(Some(msg));
                    loading.set(false);
                }
            }
        });
    };

    view! {
        <div class="min-h-screen flex items-center justify-center">
            <form
                on:submit=submit
                class="bg-zinc-900 border border-zinc-800 rounded-lg p-8 w-full max-w-sm space-y-4"
            >
                <h1 class="text-xl font-bold text-zinc-100">"Norylum — Administration"</h1>
                {move || error.get().map(|e| view! {
                    <p class="text-red-400 text-sm">{e}</p>
                })}
                <div>
                    <label class="block text-xs text-zinc-400 mb-1">"Nom d'utilisateur"</label>
                    <input
                        type="text"
                        class="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-zinc-100 text-sm focus:outline-none focus:border-zinc-500"
                        on:input=move |e| username.set(event_target_value(&e))
                        prop:value=username
                    />
                </div>
                <div>
                    <label class="block text-xs text-zinc-400 mb-1">"Mot de passe"</label>
                    <input
                        type="password"
                        class="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-zinc-100 text-sm focus:outline-none focus:border-zinc-500"
                        on:input=move |e| password.set(event_target_value(&e))
                        prop:value=password
                    />
                </div>
                <button
                    type="submit"
                    disabled=move || loading.get()
                    class="w-full bg-zinc-100 text-zinc-900 rounded py-2 text-sm font-medium hover:bg-white disabled:opacity-50"
                >
                    {move || if loading.get() { "Connexion…" } else { "Se connecter" }}
                </button>
            </form>
        </div>
    }
}
