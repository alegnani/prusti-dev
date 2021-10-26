// © 2021, ETH Zurich
//
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

#![feature(rustc_private)]
#![feature(box_patterns)]

extern crate rustc_data_structures;
extern crate rustc_middle;
extern crate rustc_index;
extern crate rustc_span;
extern crate serde;

pub mod domains;
mod abstract_state;
mod analysis_error;
mod analysis;
mod pointwise_state;
mod serialization_utils;

pub use abstract_state::AbstractState;
pub use analysis_error::AnalysisError;
pub use analysis::Analysis;
pub use pointwise_state::PointwiseState;
