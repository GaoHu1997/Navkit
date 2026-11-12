//! Shared access to the actual managed application state; this is not an execution engine.
//! Desktop borrows keep the tauri::State guard alive. Owned services keep the same Arc<AppState>.
use std::{ops::Deref, sync::Arc};
use tauri::Manager;
use crate::commands::AppState;

#[derive(Clone)]
pub(crate) enum SchedulerStateAccess {
    Desktop(tauri::AppHandle),
    Owned(Arc<AppState>),
}

pub(crate) enum SchedulerStateGuard<'a> {
    Desktop(tauri::State<'a, AppState>),
    Owned(&'a AppState),
}

impl SchedulerStateAccess {
    pub(crate) fn get(&self) -> SchedulerStateGuard<'_> {
        match self {
            Self::Desktop(app) => SchedulerStateGuard::Desktop(app.state::<AppState>()),
            Self::Owned(state) => SchedulerStateGuard::Owned(state.as_ref()),
        }
    }
}

impl Deref for SchedulerStateGuard<'_> {
    type Target = AppState;
    fn deref(&self) -> &Self::Target {
        match self {
            Self::Desktop(state) => state.inner(),
            Self::Owned(state) => state,
        }
    }
}
