script {
    use aptos_framework::coin;
    use aptos_framework::aptos_coin::AptosCoin;
    use aptos_framework::fungible_asset;

    const ALICE_ADDRESS: address = @0xf5b9d6f01a99e74c790e2f330c092fa05455a8193f1dfc1b113ecc54d067afe1;
    const BOB_ADDRESS: address = @0x3abd8bfba992bf0dbff06518a52421b1a1f7bda5a6f32973aaa31a8bcb866042;

    fun main(account: &signer) {
        for (i in 0..500) {
            coin::transfer<AptosCoin>(account, BOB_ADDRESS, 1000 + i * 2);
        };
    }
}
