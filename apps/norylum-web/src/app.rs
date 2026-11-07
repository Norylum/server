use leptos::prelude::*;
use leptos_router::{
    components::{Route, Router, Routes},
    path,
};

use crate::pages::{acquisitions::Acquisitions, dashboard::Dashboard, login::Login, media::Media, seeder::Seeder};

#[component]
pub fn App() -> impl IntoView {
    view! {
        <Router>
            <Routes fallback=|| view! { <p class="p-8 text-zinc-400">"Page introuvable"</p> }>
                <Route path=path!("/login") view=Login/>
                <Route path=path!("/") view=Dashboard/>
                <Route path=path!("/media") view=Media/>
                <Route path=path!("/acquisitions") view=Acquisitions/>
                <Route path=path!("/seeder") view=Seeder/>
            </Routes>
        </Router>
    }
}
