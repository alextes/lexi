# relay dynamics

this document aims to explain what the ultra sound relay does and how it works. it may be useful to answer questions about the relay, bids, headers, block builders, node operators, and proposers.

## winning headers

the relay's main goal is to every slot take in thousands of bids from block builders and select the best one. this bid is offered to the proposer. the prooposer will then select the best bid from many relays. the `auction_analysis` table in mevdb shows for each slot which bid won if any. the `winning_block_hash` column shows the block hash of the winning bid. the `turbo_header_requests` table in mevdb shows the bids offered for each slot to the proposer by the ultra sound relay. most slots have only one header offered by our relay, but some have multiple. ultra sound relay won if the `turbo_header_requests` table has a `block_hash` matching the `winning_block_hash` in the `auction_analysis` table. another way to determine if ultra sound relay won is to check the `auction_analysis` table for the `relays` column, value `ultra-sound`.

## demotions

in order to process bids faster, some bids are processed optimistically. this means the builder bid is not checked for validity before being selected. when checking afterwards whether the bid is valid, if the bid was invalid, the builder is demoted from optimistic status. a builder's current optimistic status can be found in the `builder` table in globaldb in the `is_optimistic` column.

## block builders

submit thousands of bids every slot to the relay to win a low latency auction. the highest bid at the time the proposer calls is offered to the proposer.

## adjustments

this is how the ultra sound relay team makes money. the relay will attempt last minute to decide whether its bid is higher than any other relay, if so, the relay will attempt to adjust its top bid value / bid header to the second highest plus episilon or 1. the delta is split 50 / 50 between the relay and the block builder. the adjusted value of a block bid may be found in the `turbo_header_requests` table in mevdb in the `adjusted_bid_value` column.

## notes on mevdb tables

`auction_analysis` is the most important table. it shows the winning bid for each slot. for all relays.
`turbo_header_requests` is the second most important table. it shows the bids offered for each slot to the proposer by the ultra sound relay.
`block_production` is a stale table. don't use it.

## geography

some tables will feature a `geo` column. ultra sound operates block auctions in multiple regions, so far `rbx` and `vin`. this column will indicate where a builder block submission was received, or where a proposer header was given out.

## proposers

when asked information about the proposer, you usually need to work from a proposer_pubkey or pubkey column in a proposer table, from there you can use `proposer_labels_with_imputed_data_view` in mevdb + the `pubkey` column to find the `label` which is the proposer or node operator name. `validators` in mevdb also contains proposer information. validator is a synonym for proposer. node operators are professional companies which manage many proposers.
