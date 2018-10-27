// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at http://mozilla.org/MPL/2.0/.

use encoder::borrows::{compute_procedure_contract, ProcedureContract, ProcedureContractMirDef};
use encoder::builtin_encoder::BuiltinEncoder;
use encoder::builtin_encoder::BuiltinMethodKind;
use encoder::builtin_encoder::BuiltinFunctionKind;
use encoder::error_manager::ErrorManager;
use encoder::mir_encoder::MirEncoder;
use encoder::spec_encoder::SpecEncoder;
use encoder::places;
use encoder::procedure_encoder::ProcedureEncoder;
use encoder::pure_function_encoder::PureFunctionEncoder;
use encoder::type_encoder::TypeEncoder;
use encoder::vir;
use prusti_interface::data::ProcedureDefId;
use prusti_interface::environment::Environment;
use prusti_interface::environment::EnvironmentImpl;
use prusti_interface::report::Log;
use rustc::hir::def_id::DefId;
use rustc::middle::const_val::ConstVal;
use rustc::hir;
use rustc::mir;
use rustc::ty;
use std::cell::{RefCell, RefMut};
use std::collections::HashMap;
use syntax::ast;
use viper;
use prusti_interface::specifications::{SpecID, TypedSpecificationMap, SpecificationSet, TypedAssertion, Specification, SpecType, Assertion};
use prusti_interface::constants::PRUSTI_SPEC_ATTR;
use std::mem;
use prusti_interface::environment::Procedure;
use std::io::Write;

pub struct Encoder<'v, 'r: 'v, 'a: 'r, 'tcx: 'a> {
    env: &'v EnvironmentImpl<'r, 'a, 'tcx>,
    spec: &'v TypedSpecificationMap,
    error_manager: RefCell<ErrorManager<'tcx>>,
    procedure_contracts: RefCell<HashMap<ProcedureDefId, ProcedureContractMirDef<'tcx>>>,
    builtin_methods: RefCell<HashMap<BuiltinMethodKind, vir::BodylessMethod>>,
    builtin_functions: RefCell<HashMap<BuiltinFunctionKind, vir::Function>>,
    procedures: RefCell<HashMap<ProcedureDefId, vir::CfgMethod>>,
    pure_functions: RefCell<HashMap<ProcedureDefId, vir::Function>>,
    type_predicate_names: RefCell<HashMap<ty::TypeVariants<'tcx>, String>>,
    predicate_types: RefCell<HashMap<String, ty::Ty<'tcx>>>,
    type_predicates: RefCell<HashMap<String, vir::Predicate>>,
    fields: RefCell<HashMap<String, vir::Field>>,
    /// For each instantiation of each closure: DefId, basic block index, statement index, operands
    closure_instantiations: HashMap<DefId, Vec<(ProcedureDefId, mir::BasicBlock, usize, Vec<mir::Operand<'tcx>>)>>,
    encoding_queue: RefCell<Vec<ProcedureDefId>>,
    vir_program_before_foldunfold_writer: RefCell<Box<Write>>,
    vir_program_before_viper_writer: RefCell<Box<Write>>,
}

impl<'v, 'r, 'a, 'tcx> Encoder<'v, 'r, 'a, 'tcx> {
    pub fn new(env: &'v EnvironmentImpl<'r, 'a, 'tcx>, spec: &'v TypedSpecificationMap) -> Self {
        let source_path = env.source_path();
        let source_filename = source_path.file_name().unwrap().to_str().unwrap();
        let vir_program_before_foldunfold_writer = RefCell::new(
            Log::writer("vir_program_before_foldunfold", format!("{}.vir", source_filename)).ok().unwrap()
        );
        let vir_program_before_viper_writer = RefCell::new(
            Log::writer("vir_program_before_viper", format!("{}.vir", source_filename)).ok().unwrap()
        );

        Encoder {
            env,
            spec,
            error_manager: RefCell::new(ErrorManager::new(env.codemap())),
            procedure_contracts: RefCell::new(HashMap::new()),
            builtin_methods: RefCell::new(HashMap::new()),
            builtin_functions: RefCell::new(HashMap::new()),
            procedures: RefCell::new(HashMap::new()),
            pure_functions: RefCell::new(HashMap::new()),
            type_predicate_names: RefCell::new(HashMap::new()),
            predicate_types: RefCell::new(HashMap::new()),
            type_predicates: RefCell::new(HashMap::new()),
            fields: RefCell::new(HashMap::new()),
            closure_instantiations: HashMap::new(),
            encoding_queue: RefCell::new(vec![]),
            vir_program_before_foldunfold_writer,
            vir_program_before_viper_writer
        }
    }

    pub fn log_vir_program_before_foldunfold<S: ToString>(&self, program: S) {
        let mut writer = self.vir_program_before_foldunfold_writer.borrow_mut();
        writer.write_all(program.to_string().as_bytes()).ok().unwrap();
        writer.write_all("\n\n".to_string().as_bytes()).ok().unwrap();
        writer.flush().ok().unwrap();
    }

    pub fn log_vir_program_before_viper<S: ToString>(&self, program: S) {
        let mut writer = self.vir_program_before_viper_writer.borrow_mut();
        writer.write_all(program.to_string().as_bytes()).ok().unwrap();
        writer.write_all("\n\n".to_string().as_bytes()).ok().unwrap();
        writer.flush().ok().unwrap();
    }

    fn initialize(&mut self) {
        self.collect_closure_instantiations();
    }

    pub fn env(&self) -> &'v EnvironmentImpl<'r, 'a, 'tcx> {
        self.env
    }

    pub fn spec(&self) -> &'v TypedSpecificationMap {
        self.spec
    }

    pub fn error_manager(&self) -> RefMut<ErrorManager<'tcx>> {
        self.error_manager.borrow_mut()
    }

    pub fn get_used_viper_domains(&self) -> Vec<viper::Domain<'v>> {
        vec![]
    }

    pub fn get_used_viper_fields(&self) -> Vec<vir::Field> {
        self.fields.borrow().values().cloned().collect()
    }

    pub fn get_used_viper_functions(&self) -> Vec<Box<vir::ToViper<'v, viper::Function<'v>>>> {
        let mut functions: Vec<Box<vir::ToViper<'v, viper::Function<'v>>>> = vec![];
        for function in self.builtin_functions.borrow().values() {
            functions.push(Box::new(function.clone()));
        }
        for function in self.pure_functions.borrow().values() {
            functions.push(Box::new(function.clone()));
        }
        functions
    }

    pub fn get_used_viper_predicates(&self) -> Vec<vir::Predicate> {
        self.type_predicates.borrow().values().cloned().collect()
    }

    pub fn get_used_viper_predicates_map(&self) -> HashMap<String, vir::Predicate> {
        self.type_predicates.borrow().clone()
    }

    pub fn get_used_viper_methods(&self) -> Vec<Box<vir::ToViper<'v, viper::Method<'v>>>> {
        let mut methods: Vec<Box<vir::ToViper<'v, viper::Method<'v>>>> = vec![];
        for method in self.builtin_methods.borrow().values() {
            methods.push(Box::new(method.clone()));
        }
        for procedure in self.procedures.borrow().values() {
            methods.push(Box::new(procedure.clone()));
        }
        methods
    }

    fn collect_closure_instantiations(&mut self) {
        debug!("Collecting closure instantiations...");
        let tcx = self.env().tcx();
        let mut closure_instantiations: HashMap<DefId, Vec<_>> = HashMap::new();
        let crate_num = hir::def_id::LOCAL_CRATE;
        for &mir_def_id in tcx.mir_keys(crate_num).iter() {
            if !(
                self.env().has_attribute_name(mir_def_id, "__PRUSTI_LOOP_SPEC_ID") ||
                self.env().has_attribute_name(mir_def_id, "__PRUSTI_EXPR_ID") ||
                self.env().has_attribute_name(mir_def_id, "__PRUSTI_FORALL_ID") ||
                self.env().has_attribute_name(mir_def_id, "__PRUSTI_SPEC_ONLY") ||
                self.env().has_attribute_name(mir_def_id, PRUSTI_SPEC_ATTR)
            ) {
                continue;
            }
            trace!("Collecting closure instantiations for mir {:?}", mir_def_id);
            let mir = tcx.mir_validated(mir_def_id).borrow();
            for (bb_index, bb_data) in mir.basic_blocks().iter_enumerated() {
                for (stmt_index, stmt) in bb_data.statements.iter().enumerate() {
                    if let mir::StatementKind::Assign(
                        _,
                        mir::Rvalue::Aggregate(
                            box mir::AggregateKind::Closure(cl_def_id, _),
                            ref operands
                        )
                    ) = stmt.kind {
                        trace!("Found closure instantiation: {:?}", stmt);
                        let instantiations = closure_instantiations.entry(cl_def_id).or_insert(vec![]);
                        instantiations.push(
                            (
                                mir_def_id,
                                bb_index,
                                stmt_index,
                                operands.clone()
                            )
                        )
                    }
                }
            }
        }
        self.closure_instantiations = closure_instantiations;
    }

    pub fn get_closure_instantiations(&self, closure_def_id: DefId) -> Vec<(ProcedureDefId, mir::BasicBlock, usize, Vec<mir::Operand<'tcx>>)> {
        trace!("Get closure instantiations for {:?}", closure_def_id);
        match self.closure_instantiations.get(&closure_def_id) {
            Some(result) => result.clone(),
            None => vec![]
        }
    }

    fn get_procedure_contract(&self, proc_def_id: ProcedureDefId) ->  ProcedureContractMirDef<'tcx> {
        let opt_spec_id: Option<SpecID> = self.env().tcx()
            .get_attrs(proc_def_id)
            .iter()
            .find(|attr| attr.check_name(PRUSTI_SPEC_ATTR))
            .and_then(|x| x
                .value_str()
                .and_then(|y| y
                    .as_str()
                    .parse::<u64>()
                    .ok()
                    .map(
                        |z| z.into()
                    )
                )
            );
        let opt_fun_spec = opt_spec_id.and_then(|spec_id| self.spec().get(&spec_id));
        let fun_spec = match opt_fun_spec {
            Some(fun_spec) => fun_spec.clone(),
            None => {
                warn!("Procedure {:?} has no specification", proc_def_id);
                // TODO: use false as precondition as default?
                SpecificationSet::Procedure(vec![], vec![])
            }
        };
        compute_procedure_contract(proc_def_id, self.env().tcx(), fun_spec)
    }

    pub fn get_procedure_contract_for_def(&self, proc_def_id: ProcedureDefId
                                          ) -> ProcedureContract<'tcx> {
        self.procedure_contracts
            .borrow_mut()
            .entry(proc_def_id)
            .or_insert_with(|| self.get_procedure_contract(proc_def_id))
            .to_def_site_contract()
    }

    pub fn get_procedure_contract_for_call(&self, proc_def_id: ProcedureDefId,
                                           args: &Vec<places::Local>, target: places::Local
                                           ) -> ProcedureContract<'tcx> {
        self.procedure_contracts
            .borrow_mut()
            .entry(proc_def_id)
            .or_insert_with(|| self.get_procedure_contract(proc_def_id))
            .to_call_site_contract(args, target)
    }

    pub fn encode_value_field(&self, ty: ty::Ty<'tcx>) -> vir::Field {
        let type_encoder = TypeEncoder::new(self, ty);
        let mut field = type_encoder.encode_value_field();
        self.fields.borrow_mut().entry(field.name.clone()).or_insert_with(|| field.clone());
        field
    }

    pub fn encode_ref_field(&self, field_name: &str, ty: ty::Ty<'tcx>) -> vir::Field {
        let type_name = self.encode_type_predicate_use(ty);
        self.fields.borrow_mut().entry(field_name.to_string()).or_insert_with(|| {
            // Do not store the name of the type in self.fields
            vir::Field::new(field_name, vir::Type::TypedRef("".to_string()))
        });
        vir::Field::new(field_name, vir::Type::TypedRef(type_name))
    }

    pub fn encode_discriminant_field(&self) -> vir::Field {
        let name = "discriminant";
        let field = vir::Field::new(name, vir::Type::Int);
        self.fields.borrow_mut().entry(name.to_string()).or_insert_with(|| field.clone());
        field
    }

    pub fn encode_builtin_method_def(&self, method_kind: BuiltinMethodKind) -> vir::BodylessMethod {
        trace!("encode_builtin_method_def({:?})", method_kind);
        if !self.builtin_methods.borrow().contains_key(&method_kind) {
            let builtin_encoder = BuiltinEncoder::new(self);
            let method = builtin_encoder.encode_builtin_method_def(method_kind);
            self.log_vir_program_before_viper(method.to_string());
            self.builtin_methods.borrow_mut().insert(method_kind.clone(), method);
        }
        self.builtin_methods.borrow()[&method_kind].clone()
    }

    pub fn encode_builtin_method_use(&self, method_kind: BuiltinMethodKind) -> String {
        trace!("encode_builtin_method_use({:?})", method_kind);
        if !self.builtin_methods.borrow().contains_key(&method_kind) {
            // Trigger encoding of definition
            self.encode_builtin_method_def(method_kind);
        }
        let builtin_encoder = BuiltinEncoder::new(self);
        builtin_encoder.encode_builtin_method_name(method_kind)
    }

    pub fn encode_builtin_function_def(&self, function_kind: BuiltinFunctionKind) -> vir::Function {
        trace!("encode_builtin_function_def({:?})", function_kind);
        if !self.builtin_functions.borrow().contains_key(&function_kind) {
            let builtin_encoder = BuiltinEncoder::new(self);
            let function = builtin_encoder.encode_builtin_function_def(function_kind.clone());
            self.log_vir_program_before_viper(function.to_string());
            self.builtin_functions.borrow_mut().insert(function_kind.clone(), function);
        }
        self.builtin_functions.borrow()[&function_kind].clone()
    }

    pub fn encode_builtin_function_use(&self, function_kind: BuiltinFunctionKind) -> String {
        trace!("encode_builtin_function_use({:?})", function_kind);
        if !self.builtin_functions.borrow().contains_key(&function_kind) {
            // Trigger encoding of definition
            self.encode_builtin_function_def(function_kind.clone());
        }
        let builtin_encoder = BuiltinEncoder::new(self);
        builtin_encoder.encode_builtin_function_name(&function_kind)
    }

    pub fn encode_procedure_use(&self, proc_def_id: ProcedureDefId) -> String {
        trace!("encode_procedure_use({:?})", proc_def_id);
        assert!(!self.env.has_attribute_name(proc_def_id, "pure"), "procedure is marked as pure: {:?}", proc_def_id);
        self.queue_encoding(proc_def_id);
        let procedure = self.env.get_procedure(proc_def_id);
        let procedure_encoder = ProcedureEncoder::new(self, &procedure);
        procedure_encoder.encode_name()
    }

    pub fn encode_procedure(&self, proc_def_id: ProcedureDefId) -> vir::CfgMethod {
        trace!("encode_procedure({:?})", proc_def_id);
        assert!(!self.env.has_attribute_name(proc_def_id, "pure"), "procedure is marked as pure: {:?}", proc_def_id);
        assert!(!self.env.has_attribute_name(proc_def_id, "trusted"), "procedure is marked as trusted: {:?}", proc_def_id);
        if !self.procedures.borrow().contains_key(&proc_def_id) {
            let procedure = self.env.get_procedure(proc_def_id);
            let procedure_encoder = ProcedureEncoder::new(self, &procedure);
            let method = procedure_encoder.encode();
            self.log_vir_program_before_viper(method.to_string());
            self.procedures.borrow_mut().insert(proc_def_id, method);
        }
        self.procedures.borrow()[&proc_def_id].clone()
    }

    pub fn encode_value_type(&self, ty: ty::Ty<'tcx>) -> vir::Type {
        let type_encoder = TypeEncoder::new(self, ty);
        type_encoder.encode_value_type()
    }

    pub fn encode_type(&self, ty: ty::Ty<'tcx>) -> vir::Type {
        let type_encoder = TypeEncoder::new(self, ty);
        type_encoder.encode_type()
    }

    pub fn encode_assertion(&self, assertion: &TypedAssertion, mir: &mir::Mir<'tcx>,
                            label: &str, encoded_args: &[vir::Expr],
                            encoded_return: Option<&vir::Expr>, targets_are_values: bool,
                            stop_at_bbi: Option<mir::BasicBlock>
    ) -> vir::Expr {
        let spec_encoder = SpecEncoder::new(self, mir, label, encoded_args, encoded_return, targets_are_values, stop_at_bbi);
        spec_encoder.encode_assertion(assertion)
    }

    pub fn encode_type_predicate_use(&self, ty: ty::Ty<'tcx>) -> String {
        if !self.type_predicate_names.borrow().contains_key(&ty.sty) {
            let type_encoder = TypeEncoder::new(self, ty);
            let result = type_encoder.encode_predicate_use();
            self.type_predicate_names.borrow_mut().insert(ty.sty.clone(), result);
            // Trigger encoding of definition
            self.encode_type_predicate_def(ty);
        }
        let predicate_name = self.type_predicate_names.borrow()[&ty.sty].clone();
        self.predicate_types.borrow_mut().insert(predicate_name.clone(), ty);
        predicate_name
    }

    pub fn get_predicate_type(&self, predicate_name: String) -> Option<ty::Ty<'tcx>> {
        self.predicate_types.borrow().get(&predicate_name).cloned()
    }

    pub fn encode_type_predicate_def(&self, ty: ty::Ty<'tcx>) -> vir::Predicate {
        let predicate_name = self.encode_type_predicate_use(ty);
        if !self.type_predicates.borrow().contains_key(&predicate_name) {
            let type_encoder = TypeEncoder::new(self, ty);
            let predicate = type_encoder.encode_predicate_def();
            self.log_vir_program_before_viper(predicate.to_string());
            self.type_predicates.borrow_mut().insert(predicate_name.clone(), predicate);
        }
        self.type_predicates.borrow()[&predicate_name].clone()
    }

    pub fn get_type_predicate_by_name(&self, predicate_name: &str) -> Option<vir::Predicate> {
        self.type_predicates.borrow().get(predicate_name).cloned()
    }

    pub fn encode_const_expr(&self, value: &ty::Const<'tcx>) -> vir::Expr {
        trace!("encode_const_expr {:?}", value);
        let scalar_value = match value.val {
            ConstVal::Value(ref value) => {
                value.to_scalar().unwrap()
            },
            x => unimplemented!("{:?}", x)
        };

        let usize_bits = mem::size_of::<usize>() * 8;

        let expr = match value.ty.sty {
            ty::TypeVariants::TyBool => scalar_value.to_bool().ok().unwrap().into(),
            ty::TypeVariants::TyInt(ast::IntTy::I8) => (scalar_value.to_bits(ty::layout::Size::from_bits(8)).ok().unwrap() as i8).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I16) => (scalar_value.to_bits(ty::layout::Size::from_bits(16)).ok().unwrap() as i16).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I32) => (scalar_value.to_bits(ty::layout::Size::from_bits(32)).ok().unwrap() as i32).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I64) => (scalar_value.to_bits(ty::layout::Size::from_bits(64)).ok().unwrap() as i64).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I128) => (scalar_value.to_bits(ty::layout::Size::from_bits(128)).ok().unwrap() as i128).into(),
            ty::TypeVariants::TyInt(ast::IntTy::Isize) => (scalar_value.to_bits(ty::layout::Size::from_bits(usize_bits as u64)).ok().unwrap() as i128).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U8) => (scalar_value.to_bits(ty::layout::Size::from_bits(8)).ok().unwrap() as u8).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U16) => (scalar_value.to_bits(ty::layout::Size::from_bits(16)).ok().unwrap() as u16).into(),
            ty::TypeVariants::TyChar |
            ty::TypeVariants::TyUint(ast::UintTy::U32) => (scalar_value.to_bits(ty::layout::Size::from_bits(32)).ok().unwrap() as u32).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U64) => (scalar_value.to_bits(ty::layout::Size::from_bits(64)).ok().unwrap() as u64).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U128) => (scalar_value.to_bits(ty::layout::Size::from_bits(128)).ok().unwrap() as u128).into(),
            ty::TypeVariants::TyUint(ast::UintTy::Usize) => (scalar_value.to_bits(ty::layout::Size::from_bits(usize_bits as u64)).ok().unwrap() as u128).into(),
            ref x => unimplemented!("{:?}", x)
        };
        debug!("encode_const_expr {:?} --> {:?}", value, expr);
        expr
    }

    pub fn encode_int_cast(&self, value: u128, ty: ty::Ty<'tcx>) -> vir::Expr {
        trace!("encode_int_cast {:?} as {:?}", value, ty);
        let usize_bits = mem::size_of::<usize>() * 8;

        let expr = match ty.sty {
            ty::TypeVariants::TyBool => (value != 0).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I8) => (value as i8).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I16) => (value as i16).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I32) => (value as i32).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I64) => (value as i64).into(),
            ty::TypeVariants::TyInt(ast::IntTy::I128) => (value as i128).into(),
            ty::TypeVariants::TyInt(ast::IntTy::Isize) => (value as isize).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U8) => (value as u8).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U16) => (value as u16).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U32) => (value as u32).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U64) => (value as u64).into(),
            ty::TypeVariants::TyUint(ast::UintTy::U128) => (value as u128).into(),
            ty::TypeVariants::TyUint(ast::UintTy::Usize) => (value as usize).into(),
            ref x => unimplemented!("{:?}", x)
        };
        debug!("encode_int_cast {:?} as {:?} --> {:?}", value, ty, expr);
        expr
    }

    pub fn encode_item_name(&self, def_id: DefId) -> String {
        self.env.get_item_name(def_id)
            .replace("::", "$")
            .replace("<", "$_")
            .replace(">", "_$")
            .replace(" ", "_")
    }

    pub fn encode_pure_function_body(&self, proc_def_id: ProcedureDefId) -> vir::Expr {
        // TODO: add caching?
        let procedure = self.env.get_procedure(proc_def_id);
        let pure_function_encoder = PureFunctionEncoder::new(self, proc_def_id, procedure.get_mir());
        pure_function_encoder.encode_body()
    }

    pub fn encode_pure_function_def(&self, proc_def_id: ProcedureDefId) -> vir::Function {
        assert!(self.env.has_attribute_name(proc_def_id, "pure"), "procedure is not marked as pure: {:?}", proc_def_id);

        if !self.pure_functions.borrow().contains_key(&proc_def_id) {
            let procedure_name = self.env().tcx().item_path_str(proc_def_id);
            let procedure = self.env.get_procedure(proc_def_id);
            let pure_function_encoder = PureFunctionEncoder::new(self, proc_def_id, procedure.get_mir());
            let is_trusted = self.env.has_attribute_name(proc_def_id, "trusted");
            let function = if is_trusted {
                pure_function_encoder.encode_bodyless_function()
            } else {
                pure_function_encoder.encode_function()
            };
            self.log_vir_program_before_viper(function.to_string());
            self.pure_functions.borrow_mut().insert(proc_def_id, function);
        }
        self.pure_functions.borrow()[&proc_def_id].clone()
    }

    pub fn encode_pure_function_use(&self, proc_def_id: ProcedureDefId) -> String {
        trace!("encode_pure_function_use({:?})", proc_def_id);
        assert!(self.env.has_attribute_name(proc_def_id, "pure"), "procedure is not marked as pure: {:?}", proc_def_id);
        self.queue_encoding(proc_def_id);
        let procedure = self.env.get_procedure(proc_def_id);
        let pure_function_encoder = PureFunctionEncoder::new(self, proc_def_id, procedure.get_mir());
        pure_function_encoder.encode_function_name()
    }

    pub fn encode_pure_function_return_type(&self, proc_def_id: ProcedureDefId) -> vir::Type {
        trace!("encode_pure_function_return_type({:?})", proc_def_id);
        assert!(self.env.has_attribute_name(proc_def_id, "pure"), "procedure is not marked as pure: {:?}", proc_def_id);
        self.queue_encoding(proc_def_id);
        let procedure = self.env.get_procedure(proc_def_id);
        let pure_function_encoder = PureFunctionEncoder::new(self, proc_def_id, procedure.get_mir());
        pure_function_encoder.encode_function_return_type()
    }

    pub fn queue_encoding(&self, proc_def_id: ProcedureDefId) {
        self.encoding_queue.borrow_mut().push(proc_def_id);
    }

    pub fn process_encoding_queue(&mut self) {
        self.initialize();
        while !self.encoding_queue.borrow().is_empty() {
            let proc_def_id = self.encoding_queue.borrow_mut().pop().unwrap();
            debug!("Encoding {:?}", proc_def_id);
            let is_pure_function = self.env.has_attribute_name(proc_def_id, "pure");
            let is_trusted = self.env.has_attribute_name(proc_def_id, "trusted");
            if is_pure_function {
                self.encode_pure_function_def(proc_def_id);
            } else {
                if is_trusted {
                    debug!("Trusted procedure will not be encoded or verified: {:?}", proc_def_id);
                } else {
                    self.encode_procedure(proc_def_id);
                }
            }
        }
    }
}
