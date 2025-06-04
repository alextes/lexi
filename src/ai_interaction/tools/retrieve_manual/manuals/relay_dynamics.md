# relay dynamics

this document aims to explain what the ultra sound relay does and how it works. it may be useful to answer questions about the relay, bids, headers, block builders, node operators, and proposers.

## winning headers

the relay's main goal is to every slot take in thousands of bids from block builders and select the best one. this bid is offered to the proposer. the prooposer will then select the best bid from many relays. the `auction_analysis` table in mevdb shows for each slot which bid won if any. the `winning_block_hash` column shows the block hash of the winning bid. the `turbo_header_requests` table in mevdb shows the bids offered for each slot to the proposer by the ultra sound relay. ultra sound relay won if it is in the `auction_analysis` table in the text[] `relays` column, value `ultra-sound`. another way to determine if ultra sound relay won is to check the `turbo_header_requests` table for the `block_hash` matching the `winning_block_hash` in the `auction_analysis` table.

## demotions

in order to process bids faster, some bids are processed optimistically. this means the builder bid is not checked for validity before being selected. when checking afterwards whether the bid is valid, if the bid was invalid, the builder is demoted from optimistic status. a builder's current optimistic status can be found in the `builder` table in globaldb in the `is_optimistic` column.

## block builders

submit thousands of bids every slot to the relay to win a low latency auction. the highest bid at the time the proposer calls is offered to the proposer.

## adjustments

this is how the ultra sound relay team makes money. the relay will attempt last minute to decide whether its bid is higher than any other relay, if so, the relay will attempt to adjust its top bid value / bid header to the second highest plus episilon or 1. the delta is split 50 / 50 between the relay and the block builder. the adjusted value of a block bid may be found in the `turbo_header_requests` table in mevdb in the `adjusted_bid_value` column.
