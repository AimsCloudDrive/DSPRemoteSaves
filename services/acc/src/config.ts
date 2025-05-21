import { MongoClientOptions } from "mongodb";

export const baseSavePath = "./saves";
export const MongonServerUrl = "mongodb://never.aims.nevermonarch.cn:57857/";
export const MongonServerAuth: undefined | MongoClientOptions["auth"] = {
  username: "root",
  password: "123456",
};
export const DBName = "DSPCloudSaves";
export const UserCollectionName = "users";

export const port = 9999;
export const serverBasePath = "/dsp.saves/api";
