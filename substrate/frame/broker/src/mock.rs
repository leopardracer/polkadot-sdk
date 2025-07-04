// This file is part of Substrate.

// Copyright (C) Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#![cfg(test)]

use crate::{test_fungibles::TestFungibles, *};
use alloc::collections::btree_map::BTreeMap;
use frame_support::{
	assert_ok, derive_impl, ensure, ord_parameter_types, parameter_types,
	traits::{
		fungible::{Balanced, Credit, Inspect, ItemOf, Mutate},
		nonfungible::Inspect as NftInspect,
		tokens::{Fortitude, Precision, Preservation},
		EitherOfDiverse, Hooks, OnUnbalanced,
	},
	PalletId,
};
use frame_system::{EnsureRoot, EnsureSignedBy};
use sp_arithmetic::Perbill;
use sp_core::{ConstU32, ConstU64, Get};
use sp_runtime::{
	traits::{BlockNumberProvider, Identity, MaybeConvert},
	BuildStorage, Saturating,
};

type Block = frame_system::mocking::MockBlock<Test>;

// Configure a mock runtime to test the pallet.
frame_support::construct_runtime!(
	pub enum Test
	{
		System: frame_system,
		Broker: crate,
	}
);

#[derive_impl(frame_system::config_preludes::TestDefaultConfig)]
impl frame_system::Config for Test {
	type Block = Block;
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum CoretimeTraceItem {
	AssignCore {
		core: CoreIndex,
		begin: u32,
		assignment: Vec<(CoreAssignment, PartsOf57600)>,
		end_hint: Option<u32>,
	},
}
use CoretimeTraceItem::*;

parameter_types! {
	pub static CoretimeTrace: Vec<(u32, CoretimeTraceItem)> = Default::default();
	pub static CoretimeCredit: BTreeMap<u64, u64> = Default::default();
	pub static CoretimeSpending: Vec<(u32, u64)> = Default::default();
	pub static CoretimeWorkplan: BTreeMap<(u32, CoreIndex), Vec<(CoreAssignment, PartsOf57600)>> = Default::default();
	pub static CoretimeUsage: BTreeMap<CoreIndex, Vec<(CoreAssignment, PartsOf57600)>> = Default::default();
	pub static CoretimeInPool: CoreMaskBitCount = 0;
}

pub struct TestCoretimeProvider;
impl CoretimeInterface for TestCoretimeProvider {
	type AccountId = u64;
	type Balance = u64;
	type RelayChainBlockNumberProvider = System;
	fn request_core_count(count: CoreIndex) {
		CoreCountInbox::<Test>::put(count);
	}
	fn request_revenue_info_at(when: RCBlockNumberOf<Self>) {
		if when > RCBlockNumberProviderOf::<Self>::current_block_number() {
			panic!(
				"Asking for revenue info in the future {:?} {:?}",
				when,
				RCBlockNumberProviderOf::<Self>::current_block_number()
			);
		}

		let mut total = 0;
		CoretimeSpending::mutate(|s| {
			s.retain(|(n, a)| {
				if *n < when as u32 {
					total += a;
					false
				} else {
					true
				}
			})
		});
		// When the credit is spent, we mint this amount back into the pot (on a real network this
		// will be a teleport).
		mint_to_pot(total);
		RevenueInbox::<Test>::put(OnDemandRevenueRecord { until: when, amount: total });
	}
	fn credit_account(who: Self::AccountId, amount: Self::Balance) {
		// When the account is credited, we burn the associated funds in their account (on a real
		// network this will be a teleport).
		CoretimeCredit::mutate(|c| c.entry(who).or_default().saturating_accrue(amount));
		burn_from_pot(amount);
	}
	fn assign_core(
		core: CoreIndex,
		begin: RCBlockNumberOf<Self>,
		assignment: Vec<(CoreAssignment, PartsOf57600)>,
		end_hint: Option<RCBlockNumberOf<Self>>,
	) {
		CoretimeWorkplan::mutate(|p| p.insert((begin as u32, core), assignment.clone()));
		let item = (
			RCBlockNumberProviderOf::<Self>::current_block_number() as u32,
			AssignCore {
				core,
				begin: begin as u32,
				assignment,
				end_hint: end_hint.map(|v| v as u32),
			},
		);
		CoretimeTrace::mutate(|v| v.push(item));
	}
}

impl TestCoretimeProvider {
	pub fn spend_instantaneous(who: u64, price: u64) -> Result<(), &'static str> {
		let mut c = CoretimeCredit::get();
		ensure!(CoretimeInPool::get() > 0, "None in pool");
		c.insert(
			who,
			c.get(&who)
				.ok_or("Account not there")?
				.checked_sub(price)
				.ok_or("Checked sub failed")?,
		);
		CoretimeCredit::set(c);
		CoretimeSpending::mutate(|v| {
			v.push((RCBlockNumberProviderOf::<Self>::current_block_number() as u32, price))
		});
		Ok(())
	}
	pub fn bump() {
		let mut pool_size = CoretimeInPool::get();
		let mut workplan = CoretimeWorkplan::get();
		let mut usage = CoretimeUsage::get();
		let now = RCBlockNumberProviderOf::<Self>::current_block_number() as u32;
		workplan.retain(|(when, core), assignment| {
			if *when <= now {
				if let Some(old_assignment) = usage.get(core) {
					if let Some(a) = old_assignment.iter().find(|i| i.0 == CoreAssignment::Pool) {
						pool_size -= (a.1 / 720) as CoreMaskBitCount;
					}
				}
				if let Some(a) = assignment.iter().find(|i| i.0 == CoreAssignment::Pool) {
					pool_size += (a.1 / 720) as CoreMaskBitCount;
				}
				usage.insert(*core, assignment.clone());
				false
			} else {
				true
			}
		});
		CoretimeInPool::set(pool_size);
		CoretimeWorkplan::set(workplan);
		CoretimeUsage::set(usage);
	}
}

parameter_types! {
	pub const TestBrokerId: PalletId = PalletId(*b"TsBroker");
}

pub struct IntoZero;
impl OnUnbalanced<Credit<u64, <Test as Config>::Currency>> for IntoZero {
	fn on_nonzero_unbalanced(credit: Credit<u64, <Test as Config>::Currency>) {
		let _ = <<Test as Config>::Currency as Balanced<_>>::resolve(&0, credit);
	}
}

ord_parameter_types! {
	pub const One: u64 = 1;
	pub const MinimumCreditPurchase: u64 = 20;
}
type EnsureOneOrRoot = EitherOfDiverse<EnsureRoot<u64>, EnsureSignedBy<One, u64>>;

// Dummy implementation which converts `TaskId` to `AccountId`.
pub struct SovereignAccountOf;
impl MaybeConvert<TaskId, u64> for SovereignAccountOf {
	fn maybe_convert(task: TaskId) -> Option<u64> {
		Some(task.into())
	}
}

impl crate::Config for Test {
	type RuntimeEvent = RuntimeEvent;
	type Currency = ItemOf<TestFungibles<(), u64, (), ConstU64<0>, ()>, (), u64>;
	type OnRevenue = IntoZero;
	type TimeslicePeriod = ConstU64<2>;
	type MaxLeasedCores = ConstU32<5>;
	type MaxReservedCores = ConstU32<5>;
	type Coretime = TestCoretimeProvider;
	type ConvertBalance = Identity;
	type WeightInfo = ();
	type PalletId = TestBrokerId;
	type AdminOrigin = EnsureOneOrRoot;
	type SovereignAccountOf = SovereignAccountOf;
	type MaxAutoRenewals = ConstU32<3>;
	type PriceAdapter = CenterTargetPrice<BalanceOf<Self>>;
	type MinimumCreditPurchase = MinimumCreditPurchase;
}

pub fn advance_to(b: u64) {
	while System::block_number() < b {
		System::set_block_number(System::block_number() + 1);
		TestCoretimeProvider::bump();
		Broker::on_initialize(System::block_number());
	}
}

pub fn advance_sale_period() {
	let sale = SaleInfo::<Test>::get().unwrap();

	let target_block_number =
		sale.region_begin as u64 * <<Test as crate::Config>::TimeslicePeriod as Get<u64>>::get();

	advance_to(target_block_number)
}

pub fn pot() -> u64 {
	balance(Broker::account_id())
}

pub fn mint_to_pot(amount: u64) {
	let imb = <Test as crate::Config>::Currency::issue(amount);
	let _ = <Test as crate::Config>::Currency::resolve(&Broker::account_id(), imb);
}

pub fn burn_from_pot(amount: u64) {
	let _ = <Test as crate::Config>::Currency::burn_from(
		&Broker::account_id(),
		amount,
		Preservation::Expendable,
		Precision::Exact,
		Fortitude::Polite,
	)
	.expect("Broker pot should have sufficient balance to burn.");
}

pub fn revenue() -> u64 {
	balance(0)
}

pub fn balance(who: u64) -> u64 {
	<<Test as Config>::Currency as Inspect<_>>::total_balance(&who)
}

pub fn attribute<T: codec::Decode>(nft: RegionId, attribute: impl codec::Encode) -> T {
	<Broker as NftInspect<_>>::typed_attribute::<_, T>(&nft.into(), &attribute).unwrap()
}

pub fn new_config() -> ConfigRecordOf<Test> {
	ConfigRecord {
		advance_notice: 2,
		interlude_length: 1,
		leadin_length: 1,
		ideal_bulk_proportion: Default::default(),
		limit_cores_offered: None,
		region_length: 3,
		renewal_bump: Perbill::from_percent(10),
		contribution_timeout: 5,
	}
}

pub fn endow(who: u64, amount: u64) {
	assert_ok!(<<Test as Config>::Currency as Mutate<_>>::mint_into(&who, amount));
}

pub struct TestExt(ConfigRecordOf<Test>);
#[allow(dead_code)]
impl TestExt {
	pub fn new() -> Self {
		Self(new_config())
	}

	pub fn new_with_config(config: ConfigRecordOf<Test>) -> Self {
		Self(config)
	}

	pub fn advance_notice(mut self, advance_notice: Timeslice) -> Self {
		self.0.advance_notice = advance_notice as u64;
		self
	}

	pub fn interlude_length(mut self, interlude_length: u64) -> Self {
		self.0.interlude_length = interlude_length;
		self
	}

	pub fn leadin_length(mut self, leadin_length: u64) -> Self {
		self.0.leadin_length = leadin_length;
		self
	}

	pub fn region_length(mut self, region_length: Timeslice) -> Self {
		self.0.region_length = region_length;
		self
	}

	pub fn ideal_bulk_proportion(mut self, ideal_bulk_proportion: Perbill) -> Self {
		self.0.ideal_bulk_proportion = ideal_bulk_proportion;
		self
	}

	pub fn limit_cores_offered(mut self, limit_cores_offered: Option<CoreIndex>) -> Self {
		self.0.limit_cores_offered = limit_cores_offered;
		self
	}

	pub fn renewal_bump(mut self, renewal_bump: Perbill) -> Self {
		self.0.renewal_bump = renewal_bump;
		self
	}

	pub fn contribution_timeout(mut self, contribution_timeout: Timeslice) -> Self {
		self.0.contribution_timeout = contribution_timeout;
		self
	}

	pub fn endow(self, who: u64, amount: u64) -> Self {
		endow(who, amount);
		self
	}

	pub fn execute_with<R>(self, f: impl Fn() -> R) -> R {
		new_test_ext().execute_with(|| {
			assert_ok!(Broker::do_configure(self.0));
			f()
		})
	}
}

pub fn new_test_ext() -> sp_io::TestExternalities {
	let c = frame_system::GenesisConfig::<Test>::default().build_storage().unwrap();
	sp_io::TestExternalities::from(c)
}
