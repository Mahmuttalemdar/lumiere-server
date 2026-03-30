.PHONY: dev test test-unit test-integration lint docker load-test load-test-ws load-test-mixed load-test-storm clean

dev:
	docker compose up -d
	cargo run --bin lumiere-server

test: test-unit test-integration

test-unit:
	cargo test --lib --all

test-integration:
	docker compose -f docker-compose.test.yml up -d
	@echo "Waiting for services..."
	@until docker compose -f docker-compose.test.yml exec -T scylladb-test cqlsh -e "SELECT now() FROM system.local" >/dev/null 2>&1; do \
		echo "  ScyllaDB not ready, retrying..."; \
		sleep 5; \
	done
	@echo "All services ready!"
	LUMIERE_ENV=test cargo test --tests -p lumiere-server -- --test-threads=1; \
	EXIT_CODE=$$?; \
	docker compose -f docker-compose.test.yml down -v; \
	exit $$EXIT_CODE

lint:
	cargo fmt --all -- --check
	cargo clippy --all-targets -- -D warnings

docker:
	docker build -t lumiere-server .

load-test:
	k6 run -e BASE_URL=http://localhost:8080 load-tests/scenarios/message_throughput.js

load-test-history:
	k6 run -e BASE_URL=http://localhost:8080 load-tests/scenarios/message_history.js

load-test-ws:
	k6 run -e BASE_URL=http://localhost:8080 load-tests/scenarios/websocket_connections.js

load-test-mixed:
	k6 run -e BASE_URL=http://localhost:8080 load-tests/scenarios/mixed_workload.js

load-test-storm:
	k6 run -e BASE_URL=http://localhost:8080 load-tests/scenarios/connection_storm.js

clean:
	docker compose -f docker-compose.test.yml down -v
	cargo clean
