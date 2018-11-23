default: client server

client: 
	tsc -p tsconfig.client.json

server:
	tsc -p tsconfig.json

run:
	npm start
