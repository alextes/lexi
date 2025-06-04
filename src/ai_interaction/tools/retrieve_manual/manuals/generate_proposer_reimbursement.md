# manual: generate proposer reimbursement

this manual outlines the steps to generate a message for proposer reimbursement when a slot is missed.

**objective:** to provide the necessary information to reimburse a proposer for a missed slot where their block was chosen by a relay but not included on-chain.

**steps:**

1.  **confirm slot was missed:**

    - the user will typically ask to generate a reimbursement message for a specific `slot_number`.
    - first, use the `check_beacon_slot_missed` tool to verify if the given `slot_number` was indeed missed.
    - **only proceed if the tool indicates the slot was 'missed'.** if it was 'not_missed' or an 'error' occurred, inform the user and stop.
    - _example scenario from walkthrough:_ for slot `11808749`, the `check_beacon_slot_missed` tool should return a status indicating it was missed (equivalent to an http 404 from the beacon node).

2.  **verify slot in `mevdb.missed_slots`:**

    - query the `mevdb` database to ensure the `slot_number` is recorded in the `missed_slots` table.
    - if the slot is not found in this table, report this to the user and stop. the reimbursement process cannot proceed without this record.
    - **sql query for `mevdb` (example for slot `11808749`):**
      ```sql
      select exists(select 1 from missed_slots where slot_number = 11808749);
      ```
    - _example scenario from walkthrough:_ this query returned `true` for slot `11808749`.

3.  **gather data from `mevdb`:**

    - if the slot is confirmed missed and present in `missed_slots`, execute the following query against `mevdb` to retrieve bid details. replace `[slot_number]` with the actual slot number.
    - pay attention to the `is_adjusted_bid` field. you will need to report whether the bid was adjusted.
    - **sql query for `mevdb` (example for slot `[slot_number]`):**
      ```sql
      with
          proposer_labels as (
              select
                  pubkey                 as proposer_pubkey,
                  string_agg(label, ',') as proposer_labels
              from
                  proposer_labels_with_imputed_data_view
              group by pubkey
          )
      select
          slot_number                                      as slot,
          relayed_block_hash                               as block_hash,
          thr.proposer_pubkey,
          coalesce(proposer_labels, 'unknown')             as proposer_labels,
          adjusted_bid_value > 0                           as is_adjusted_bid, -- important for reporting
          bid_value,
          round(bid_value / 1e18, 3)                       as bid_value_eth,
          10000000000000000                                as penalty,
          round(10000000000000000 / 1e18, 3)::text         as penalty_eth,
          10000000000000000 + bid_value                    as total,
          round((10000000000000000 + bid_value) / 1e18, 3) as total_eth
      from
          missed_slots ms
          left join turbo_header_requests thr on ms.relayed_block_hash = thr.block_hash and ms.slot_number = thr.slot
          left join proposer_labels pl on thr.proposer_pubkey = pl.proposer_pubkey
      where
          slot_number = [slot_number];
      ```
    - _example data from walkthrough for slot `11808749` yielded:_
      - `slot`: `11808749`
      - `block_hash`: `0x4a4f2b9059117e102d33f1e9da145c55ec22fc49d056b3ac5ed9bd573944dcf0`
      - `proposer_pubkey`: `0xb060013948f1c110d56d877fec2be8b44ab78ace8923d38107609ef453a7ccad1950da587d49b2ca91ddef44c857f3f3`
      - `proposer_labels`: `staking facilities`
      - `is_adjusted_bid`: `t` (true)
      - `bid_value`: `300598292634102952`
      - `bid_value_eth`: `0.30059829263410295200`
      - `penalty`: `10000000000000000`
      - `penalty_eth`: `0.010`
      - `total`: `310598292634102952`
      - `total_eth`: `0.311`

4.  **gather data from `globaldb`:**

    - using the `block_hash` obtained from the `mevdb` query in the previous step, query the `globaldb` to find the `proposer_fee_recipient`. replace `[block_hash]` with the actual block hash.
    - **sql query for `globaldb` (example for `block_hash`):**
      ```sql
      select
          proposer_fee_recipient
      from
          block_submission
      where block_hash = '[block_hash]'
      limit 1;
      ```
    - example data from walkthrough for block_hash `0x4a4f2b9059117e102d33f1e9da145c55ec22fc49d056b3ac5ed9bd573944dcf0` yielded:
      - `proposer_fee_recipient`: `0x388c818ca8b9251b393131c08a736a67ccb19297`

5.  **assemble the reimbursement message:**

    - once all data is collected, inform the user whether the bid associated with the missed slot was adjusted or not (based on the `is_adjusted_bid` field from the `mevdb` query). for example: "the bid for this missed slot was adjusted." or "the bid for this missed slot was not adjusted." (in our walkthrough example, it was adjusted).
    - then, provide the reimbursement details in a telegram-friendly code block using the following format. substitute the `$variable` placeholders with the actual data retrieved.

    ```text
    slot: $slot
    block_hash: $block_hash
    proposer_pubkey: $proposer_pubkey
    bid: $bid_value ($bid_value_eth eth)
    penalty: $penalty ($penalty_eth eth)
    total: $total ($total_eth eth)
    fee_recipient: $fee_recipient
    proposer_operator: $proposer_labels
    ```

    - _example assembled code block from walkthrough:_
      ```text
      slot: 11808749
      block_hash: 0x4a4f2b9059117e102d33f1e9da145c55ec22fc49d056b3ac5ed9bd573944dcf0
      proposer_pubkey: 0xb060013948f1c110d56d877fec2be8b44ab78ace8923d38107609ef453a7ccad1950da587d49b2ca91ddef44c857f3f3
      bid: 300598292634102952 (0.30059829263410295200 eth)
      penalty: 10000000000000000 (0.010 eth)
      total: 310598292634102952 (0.311 eth)
      fee_recipient: 0x388c818ca8b9251b393131c08a736a67ccb19297
      proposer_operator: staking facilities
      ```

**summary of tools to use:**

- `check_beacon_slot_missed` (for step 1)
- `execute_mevdb_query` (for steps 2 and 3)
- `execute_globaldb_query` (for step 4)

always ensure you replace placeholders like `[slot_number]` and `[block_hash]` with the actual values from the user's request or previous query results.
