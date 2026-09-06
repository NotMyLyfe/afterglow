## Roadmap:

As of now (2026-09-06), a basic scaffold of the project has been established.
The following roadmap outlines planned features and improvements for the project.

### Phase 2: Failover and Consensus Mechanisms

This phase will focus on dynamically selecting a primary database for write operations.

- [ ] Database health detection and logging
- [ ] Move primary identity from static config to shared, watchable store
- [ ] Manual promotion of replicas to primary
- [ ] Health observation in shared store for primary election
- [ ] Quorum agreement on primary failure
  - Negative test should be included: if primary is still up, but minority of observers lose path to primary, then primary should not be demoted
- [ ] Controller leader election
- [ ] Automatic promotion of database to primary based on health, quorum, and up-to-date status
- [ ] Fence mechanism to prevent split-brain scenarios
  - Must test partitioning scenarios, not just dead databases since dead primaries can't split-brain

### Phase 3: Distributed Proxies

This phase will focus on implementing multiple proxies that can be deployed, where each proxy are co-located and share a 1:N relationship with their respective databases.

Proxies communicate with each other, with writes forwarding a singular primary database in the entire cluster, and PostgreSQL's built-in replication mechanism to propagate them to the other database to serve as read replicas. The rationale for this is to protect the number of connections to the primary database.

Amongst all proxies, the primary proxy contains the primary database and is responsible for handling writes containing the primary database in addition to any other read replicas. All other proxies contain a single relay database from the primary proxy, which fans out reads to all other databases within their co-location. This protects the primary proxy's bandwidth and allows for a more scalable architecture.

Primary identity and topology of the cluster is stored in a shared, watchable store. Consistency floor is carried by the client via causal tokens.

At WAN scale, fixed wait times for replication becomes expensive where replicas may fall behind the primary database. Proxies will need to wait the timeout, fail, and pay the cost of a round trip to the primary proxy to get the latest data. Adaptive waiting based on observed lag reduces cost.

#### Open Questions:

- Initial primary placement
- How to select a primary database - failover is two-tiered: prioritize caught-up databases, then need some priority mechanism to select a primary database from amongst the caught-up databases
- Cross-unit transactions - transactions from remote require pinning of connections when forwarding to the primary proxy
