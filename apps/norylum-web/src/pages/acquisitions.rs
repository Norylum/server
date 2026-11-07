use leptos::prelude::*;

use crate::api::{
    approve_acquisition, cancel_acquisition, get_token, list_acquisitions, register_source,
    RegisterSourcePayload,
};
use crate::components::{nav::Nav, status_badge::StatusBadge};

#[component]
pub fn Acquisitions() -> impl IntoView {
    if get_token().is_none() {
        let _ = web_sys::window()
            .and_then(|w| w.location().assign("/login").ok());
        return view! { <></> }.into_any();
    }

    let requests = Resource::new(|| (), |_| list_acquisitions());
    let feedback = RwSignal::new(Option::<String>::None);

    let do_approve = move |id: String| {
        leptos::task::spawn_local(async move {
            match approve_acquisition(&id).await {
                Ok(_) => feedback.set(Some("Approuvé.".into())),
                Err(e) => feedback.set(Some(e)),
            }
        });
    };

    let do_cancel = move |id: String| {
        leptos::task::spawn_local(async move {
            match cancel_acquisition(&id).await {
                Ok(_) => feedback.set(Some("Annulé.".into())),
                Err(e) => feedback.set(Some(e)),
            }
        });
    };

    // Source form state
    let source_id = RwSignal::new(String::new());
    let source_url = RwSignal::new(String::new());
    let source_sha = RwSignal::new(String::new());
    let source_hosts = RwSignal::new(String::new());

    let do_source = move |ev: web_sys::SubmitEvent| {
        ev.prevent_default();
        let id = source_id.get();
        let url = source_url.get();
        let sha = source_sha.get();
        let hosts_raw = source_hosts.get();
        leptos::task::spawn_local(async move {
            let hosts: Vec<&str> = hosts_raw.split(',').map(str::trim).collect();
            let payload = RegisterSourcePayload {
                source_url: &url,
                expected_sha256: &sha,
                allowed_hosts: hosts,
            };
            match register_source(&id, &payload).await {
                Ok(_) => feedback.set(Some("Source enregistrée.".into())),
                Err(e) => feedback.set(Some(e)),
            }
        });
    };

    view! {
        <div>
            <Nav/>
            <main class="p-8 space-y-6">
                <h1 class="text-2xl font-bold text-zinc-100">"Acquisitions"</h1>
                {move || feedback.get().map(|msg| view! {
                    <p class="text-sm text-zinc-300 bg-zinc-800 px-3 py-2 rounded">{msg}</p>
                })}
                {move || match requests.get() {
                    None => view! { <p class="text-zinc-500">"Chargement…"</p> }.into_any(),
                    Some(Err(e)) => view! { <p class="text-red-400">{e}</p> }.into_any(),
                    Some(Ok(items)) => view! {
                        <div class="overflow-x-auto">
                            <table class="w-full text-sm text-left">
                                <thead class="text-xs text-zinc-500 uppercase border-b border-zinc-800">
                                    <tr>
                                        <th class="pb-2 pr-4">"ID"</th>
                                        <th class="pb-2 pr-4">"Type"</th>
                                        <th class="pb-2 pr-4">"TMDB"</th>
                                        <th class="pb-2 pr-4">"Statut"</th>
                                        <th class="pb-2">"Actions"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {items.into_iter().map(|req| {
                                        let id_approve = req.id.clone();
                                        let id_cancel = req.id.clone();
                                        let do_approve = do_approve.clone();
                                        let do_cancel = do_cancel.clone();
                                        view! {
                                            <tr class="border-b border-zinc-900 hover:bg-zinc-900/50">
                                                <td class="py-2 pr-4 font-mono text-xs text-zinc-500">{req.id.clone()}</td>
                                                <td class="py-2 pr-4 text-zinc-300">{req.kind}</td>
                                                <td class="py-2 pr-4 text-zinc-300">{req.tmdb_id.to_string()}</td>
                                                <td class="py-2 pr-4">
                                                    <StatusBadge status=req.status/>
                                                </td>
                                                <td class="py-2 flex gap-2">
                                                    <button
                                                        class="text-xs px-2 py-1 rounded bg-green-900 hover:bg-green-800 text-green-200"
                                                        on:click=move |_| do_approve(id_approve.clone())
                                                    >"Approuver"</button>
                                                    <button
                                                        class="text-xs px-2 py-1 rounded bg-red-900 hover:bg-red-800 text-red-200"
                                                        on:click=move |_| do_cancel(id_cancel.clone())
                                                    >"Annuler"</button>
                                                </td>
                                            </tr>
                                        }
                                    }).collect::<Vec<_>>()}
                                </tbody>
                            </table>
                        </div>
                    }.into_any(),
                }}
                <section class="bg-zinc-900 border border-zinc-800 rounded-lg p-5 space-y-3">
                    <h2 class="text-sm font-semibold text-zinc-300">"Enregistrer une source"</h2>
                    <form on:submit=do_source class="space-y-2">
                        <input
                            class="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-zinc-100 text-sm"
                            placeholder="ID de la demande (acq_...)"
                            on:input=move |e| source_id.set(event_target_value(&e))
                            prop:value=source_id
                        />
                        <input
                            class="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-zinc-100 text-sm"
                            placeholder="URL source (https://...)"
                            on:input=move |e| source_url.set(event_target_value(&e))
                            prop:value=source_url
                        />
                        <input
                            class="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-zinc-100 text-sm"
                            placeholder="SHA-256 attendu (64 hex)"
                            on:input=move |e| source_sha.set(event_target_value(&e))
                            prop:value=source_sha
                        />
                        <input
                            class="w-full bg-zinc-800 border border-zinc-700 rounded px-3 py-2 text-zinc-100 text-sm"
                            placeholder="Hôtes autorisés (séparés par virgule)"
                            on:input=move |e| source_hosts.set(event_target_value(&e))
                            prop:value=source_hosts
                        />
                        <button
                            type="submit"
                            class="bg-zinc-100 text-zinc-900 rounded px-4 py-2 text-sm font-medium hover:bg-white"
                        >"Enregistrer"</button>
                    </form>
                </section>
            </main>
        </div>
    }.into_any()
}
